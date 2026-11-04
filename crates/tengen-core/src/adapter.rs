//! The interface between Tengen and a Go server.
//!
//! Each server (OGS, KGS, IGS, Fox) gets one adapter crate that
//! translates its protocol into these shared types. The interface never
//! exposes a server's wire format, so the UI treats every server the same.

use std::future::Future;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::board::Color;
use crate::game::Move;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Server {
    #[serde(rename = "OGS")]
    Ogs,
    #[serde(rename = "Fox")]
    Fox,
    #[serde(rename = "KGS")]
    Kgs,
    /// Pandanet's Internet Go Server.
    #[serde(rename = "IGS")]
    Igs,
}

impl Server {
    pub fn name(self) -> &'static str {
        match self {
            Server::Ogs => "OGS",
            Server::Fox => "Fox",
            Server::Kgs => "KGS",
            Server::Igs => "IGS",
        }
    }
}

/// A login for one server. Stored in the macOS Keychain, never on disk.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// The signed-in player on a server. Ranks stay in the server's own notation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub server: Server,
    pub id: String,
    pub username: String,
    pub rank: Option<String>,
}

/// A finished or ongoing game listed by a server, enough to show a row and
/// fetch the full record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSummary {
    pub server: Server,
    pub id: String,
    pub black: String,
    pub white: String,
    pub size: u8,
    pub result: Option<String>,
    /// ISO 8601 date-time when the game started, when the server gives one.
    pub started: Option<String>,
}

/// Board size and time settings for automatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchRequest {
    pub size: u8,
    pub speed: Speed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    Blitz,
    Rapid,
    Live,
    Correspondence,
}

/// What a server tells Tengen while connected. Every adapter emits only these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerEvent {
    GameStarted {
        game_id: String,
        you: Color,
        black: String,
        white: String,
        size: u8,
        komi: f32,
    },
    Moved {
        game_id: String,
        color: Color,
        mv: Move,
    },
    Clock {
        game_id: String,
        black_ms: u64,
        white_ms: u64,
        black_periods: Option<u8>,
        white_periods: Option<u8>,
    },
    GameEnded {
        game_id: String,
        result: String,
    },
    Disconnected {
        reason: String,
    },
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("wrong username or password")]
    BadCredentials,
    #[error("not signed in")]
    NotSignedIn,
    #[error("game {0} not found")]
    GameNotFound(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("the server sent something unexpected: {0}")]
    Protocol(String),
    #[error("not supported by this server yet")]
    Unsupported,
}

/// Implemented once per server.
///
/// Realtime play (automatch, moves, clocks) will join this trait when the
/// OGS websocket work lands; records and accounts come first.
pub trait ServerAdapter: Send + Sync {
    fn server(&self) -> Server;

    fn sign_in(
        &mut self,
        credentials: &Credentials,
    ) -> impl Future<Output = Result<Account, AdapterError>> + Send;

    /// Recent games of a player, newest first.
    fn recent_games(
        &self,
        player_id: &str,
    ) -> impl Future<Output = Result<Vec<GameSummary>, AdapterError>> + Send;

    /// The full record of a game as SGF text, ready to save or load.
    fn download_sgf(
        &self,
        game_id: &str,
    ) -> impl Future<Output = Result<String, AdapterError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_never_print_the_password() {
        let c = Credentials {
            username: "rafael".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn events_serialize_for_the_interface() {
        let e = ServerEvent::GameEnded {
            game_id: "1".into(),
            result: "W+R".into(),
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"type":"gameEnded","game_id":"1","result":"W+R"}"#
        );
    }
}
