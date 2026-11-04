//! Adapter for [Online-Go.com](https://online-go.com) (OGS).
//!
//! - Records and player listings use OGS's public REST API, so SGF downloads
//!   work without signing in.
//! - [`auth`]: signing in with OAuth2 and PKCE in the browser (OGS does not
//!   offer a password login for apps).
//! - [`realtime`]: automatch and live play over the OGS websocket.

pub mod auth;
pub mod realtime;

use serde::Deserialize;
use tengen_core::adapter::{
    Account, AdapterError, Credentials, GameSummary, Server, ServerAdapter,
};

pub const DEFAULT_BASE_URL: &str = "https://online-go.com";

#[derive(Clone)]
pub struct OgsAdapter {
    base: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl Default for OgsAdapter {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl OgsAdapter {
    pub fn new(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Tengen/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        OgsAdapter {
            base: base_url.trim_end_matches('/').to_string(),
            http,
            token: None,
        }
    }

    /// Uses an OAuth2 access token obtained elsewhere.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    pub fn is_signed_in(&self) -> bool {
        self.token.is_some()
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The signed-in account and the key (`user_jwt`) for the live server,
    /// from `/api/v1/ui/config`.
    pub async fn live_credentials(&self) -> Result<(Account, String), AdapterError> {
        #[derive(Deserialize)]
        struct UiConfig {
            user_jwt: Option<String>,
            user: Option<OgsPlayer>,
        }
        let token = self.token.as_deref().ok_or(AdapterError::NotSignedIn)?;
        let resp = self
            .http
            .get(format!("{}/api/v1/ui/config", self.base))
            .bearer_auth(token)
            .send()
            .await
            .map_err(net)?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AdapterError::NotSignedIn);
        }
        let cfg: UiConfig = resp
            .error_for_status()
            .map_err(net)?
            .json()
            .await
            .map_err(proto)?;
        let (Some(jwt), Some(user)) = (cfg.user_jwt.filter(|j| !j.is_empty()), cfg.user) else {
            return Err(AdapterError::NotSignedIn);
        };
        let account = Account {
            server: Server::Ogs,
            id: user.id.to_string(),
            username: user.username,
            rank: user.ranking.map(rank_name),
        };
        Ok((account, jwt))
    }

    /// When a game started (RFC 3339), from `/api/v1/games/<id>`.
    pub async fn game_started(&self, game_id: &str) -> Option<String> {
        #[derive(Deserialize)]
        struct Detail {
            started: Option<String>,
        }
        let resp = self
            .http
            .get(format!("{}/api/v1/games/{}", self.base, game_id.trim()))
            .send()
            .await
            .ok()?;
        resp.error_for_status()
            .ok()?
            .json::<Detail>()
            .await
            .ok()?
            .started
    }

    /// The signed-in account, read from `/api/v1/me/`.
    pub async fn me(&self) -> Result<Account, AdapterError> {
        let token = self.token.as_deref().ok_or(AdapterError::NotSignedIn)?;
        let resp = self
            .http
            .get(format!("{}/api/v1/me/", self.base))
            .bearer_auth(token)
            .send()
            .await
            .map_err(net)?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AdapterError::NotSignedIn);
        }
        let me: OgsPlayer = resp
            .error_for_status()
            .map_err(net)?
            .json()
            .await
            .map_err(proto)?;
        Ok(Account {
            server: Server::Ogs,
            id: me.id.to_string(),
            username: me.username,
            rank: me.ranking.map(rank_name),
        })
    }

    fn sgf_url(&self, game_id: &str) -> String {
        format!("{}/api/v1/games/{}/sgf", self.base, game_id.trim())
    }
}

impl ServerAdapter for OgsAdapter {
    fn server(&self) -> Server {
        Server::Ogs
    }

    async fn sign_in(&mut self, _credentials: &Credentials) -> Result<Account, AdapterError> {
        // OGS signs apps in through the browser (OAuth2 + PKCE), not with a password.
        if self.token.is_some() {
            self.me().await
        } else {
            Err(AdapterError::Unsupported)
        }
    }

    async fn recent_games(&self, player_id: &str) -> Result<Vec<GameSummary>, AdapterError> {
        let url = format!(
            "{}/api/v1/players/{}/games/?ordering=-ended",
            self.base,
            player_id.trim()
        );
        let resp = self.http.get(url).send().await.map_err(net)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AdapterError::Protocol(format!(
                "player {player_id} not found"
            )));
        }
        let page: Page = resp
            .error_for_status()
            .map_err(net)?
            .json()
            .await
            .map_err(proto)?;
        Ok(page.results.into_iter().map(OgsGame::summary).collect())
    }

    async fn download_sgf(&self, game_id: &str) -> Result<String, AdapterError> {
        let resp = self
            .http
            .get(self.sgf_url(game_id))
            .send()
            .await
            .map_err(net)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AdapterError::GameNotFound(game_id.to_string()));
        }
        let text = resp
            .error_for_status()
            .map_err(net)?
            .text()
            .await
            .map_err(net)?;
        if !text.trim_start().starts_with('(') {
            return Err(AdapterError::Protocol(
                "response was not an SGF record".into(),
            ));
        }
        Ok(text)
    }
}

/// Accepts a bare game id or any OGS game link (`online-go.com/game/12345`).
pub fn parse_game_ref(input: &str) -> Option<String> {
    let s = input.trim().trim_end_matches('/');
    let last = s.rsplit('/').next()?;
    let id: String = last.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!id.is_empty()).then_some(id)
}

/// OGS ranks are a number: 30 = 1d boundary, below is kyu.
pub(crate) fn rank_name(ranking: f64) -> String {
    let r = ranking.floor() as i32;
    if r >= 30 {
        format!("{}d", r - 29)
    } else {
        format!("{}k", 30 - r)
    }
}

fn net(e: reqwest::Error) -> AdapterError {
    AdapterError::Network(e.to_string())
}

fn proto(e: reqwest::Error) -> AdapterError {
    AdapterError::Protocol(e.to_string())
}

#[derive(Deserialize)]
struct Page {
    results: Vec<OgsGame>,
}

#[derive(Deserialize)]
struct OgsPlayer {
    id: u64,
    username: String,
    ranking: Option<f64>,
}

#[derive(Deserialize)]
struct OgsGame {
    id: u64,
    width: Option<u8>,
    players: Option<OgsPlayers>,
    outcome: Option<String>,
    black_lost: Option<bool>,
    white_lost: Option<bool>,
    started: Option<String>,
}

#[derive(Deserialize)]
struct OgsPlayers {
    black: Option<OgsName>,
    white: Option<OgsName>,
}

#[derive(Deserialize)]
struct OgsName {
    username: String,
}

impl OgsGame {
    fn summary(self) -> GameSummary {
        let name = |p: Option<&OgsName>, fallback: &str| {
            p.map(|p| p.username.clone())
                .unwrap_or_else(|| fallback.to_string())
        };
        let winner = match (self.black_lost, self.white_lost) {
            (Some(false), Some(true)) => Some("B"),
            (Some(true), Some(false)) => Some("W"),
            _ => None,
        };
        let result = match (winner, self.outcome.as_deref().filter(|o| !o.is_empty())) {
            (Some(w), Some(o)) => Some(format!("{w}+{}", short_outcome(o))),
            (Some(w), None) => Some(format!("{w}+")),
            _ => None,
        };
        GameSummary {
            server: Server::Ogs,
            id: self.id.to_string(),
            black: name(
                self.players.as_ref().and_then(|p| p.black.as_ref()),
                "Black",
            ),
            white: name(
                self.players.as_ref().and_then(|p| p.white.as_ref()),
                "White",
            ),
            size: self.width.unwrap_or(19),
            result,
            started: self.started,
        }
    }
}

fn short_outcome(o: &str) -> String {
    match o {
        "Resignation" => "R".into(),
        "Timeout" => "T".into(),
        o => o.trim_end_matches(" points").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_game_links_and_ids() {
        assert_eq!(parse_game_ref("12345").as_deref(), Some("12345"));
        assert_eq!(
            parse_game_ref("https://online-go.com/game/76543210/").as_deref(),
            Some("76543210")
        );
        assert_eq!(
            parse_game_ref("https://online-go.com/game/view/42").as_deref(),
            Some("42")
        );
        assert_eq!(parse_game_ref("not a game"), None);
    }

    #[test]
    fn builds_the_sgf_url() {
        let a = OgsAdapter::new("https://online-go.com/");
        assert_eq!(
            a.sgf_url(" 42 "),
            "https://online-go.com/api/v1/games/42/sgf"
        );
    }

    #[test]
    fn names_ranks_like_ogs() {
        assert_eq!(rank_name(29.4), "1k");
        assert_eq!(rank_name(30.2), "1d");
        assert_eq!(rank_name(14.0), "16k");
    }

    #[test]
    fn summarises_a_listed_game() {
        let page: Page = serde_json::from_str(
            r#"{"results":[{"id":7,"width":19,"outcome":"Resignation","black_lost":false,"white_lost":true,
                "started":"2026-11-01T20:00:00Z","players":{"black":{"username":"rafael"},"white":{"username":"bot"}}}]}"#,
        )
        .unwrap();
        let s = page.results.into_iter().next().unwrap().summary();
        assert_eq!(
            (s.black.as_str(), s.white.as_str(), s.result.as_deref()),
            ("rafael", "bot", Some("B+R"))
        );
    }

    /// Hits the real server: `cargo test -p tengen-ogs -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn downloads_a_real_record() {
        let sgf = OgsAdapter::default().download_sgf("1").await.unwrap();
        let game = tengen_core::sgf::parse(&sgf).unwrap();
        assert!(game.info.size >= 9);
    }
}
