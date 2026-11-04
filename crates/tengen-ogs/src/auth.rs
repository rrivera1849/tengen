//! Signing in to OGS with OAuth2 and PKCE, the flow OGS asks desktop apps to use.
//!
//! 1. [`Pkce::new`] makes a one-time secret; [`authorize_url`] is opened in the
//!    browser, where the player signs in on online-go.com itself.
//! 2. OGS sends the browser back to [`REDIRECT_URI`], a tiny local web page
//!    served by [`wait_for_code`] for the duration of the sign-in.
//! 3. [`exchange_code`] trades the code for tokens; [`refresh`] renews them.
//!
//! Tengen never sees the player's OGS password.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tengen_core::adapter::AdapterError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The local port OGS redirects to. Registered with the OGS application.
pub const CALLBACK_PORT: u16 = 47613;
pub const REDIRECT_URI: &str = "http://127.0.0.1:47613/oauth/callback";

/// The public client id of Tengen's OGS application. Public clients have no
/// secret, so the id can live in the source. Set `TENGEN_OGS_CLIENT_ID` when
/// building to use a different registration.
pub fn client_id() -> Option<&'static str> {
    option_env!("TENGEN_OGS_CLIENT_ID").or(DEFAULT_CLIENT_ID).filter(|id| !id.is_empty())
}

/// Fill in once Tengen is registered at online-go.com/oauth2/applications/.
const DEFAULT_CLIENT_ID: Option<&str> = None;

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    /// Random value echoed back by OGS, to reject callbacks we didn't start.
    pub state: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_token(32);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge, state: random_token(16) }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds when the access token stops working.
    pub expires_at: i64,
}

impl Tokens {
    /// True when the access token expires within a day, so it is renewed early.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at - chrono::Utc::now().timestamp() < 24 * 3600
    }
}

pub fn authorize_url(base: &str, client_id: &str, pkce: &Pkce) -> String {
    let mut url = reqwest::Url::parse(&format!("{base}/oauth2/authorize/")).expect("valid base url");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", "read write")
        .append_pair("state", &pkce.state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

pub async fn exchange_code(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens, AdapterError> {
    token_request(http, base, &[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT_URI),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ])
    .await
}

pub async fn refresh(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<Tokens, AdapterError> {
    token_request(http, base, &[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ])
    .await
}

async fn token_request(http: &reqwest::Client, base: &str, form: &[(&str, &str)]) -> Result<Tokens, AdapterError> {
    #[derive(Deserialize)]
    struct Reply {
        access_token: String,
        refresh_token: String,
        expires_in: i64,
    }
    let resp = http
        .post(format!("{base}/oauth2/token/"))
        .form(form)
        .send()
        .await
        .map_err(|e| AdapterError::Network(e.to_string()))?;
    if resp.status().is_client_error() {
        return Err(AdapterError::BadCredentials);
    }
    let r: Reply = resp
        .error_for_status()
        .map_err(|e| AdapterError::Network(e.to_string()))?
        .json()
        .await
        .map_err(|e| AdapterError::Protocol(e.to_string()))?;
    Ok(Tokens {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_at: chrono::Utc::now().timestamp() + r.expires_in,
    })
}

/// Serves the callback page until OGS redirects back with a code, or until
/// `timeout` passes (the player closed the browser tab, say).
pub async fn wait_for_code(expected_state: &str, timeout: std::time::Duration) -> Result<String, AdapterError> {
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .await
        .map_err(|e| AdapterError::Network(format!("couldn't open the sign-in page on port {CALLBACK_PORT}: {e}")))?;
    tokio::time::timeout(timeout, async {
        loop {
            let (mut stream, _) = listener.accept().await.map_err(|e| AdapterError::Network(e.to_string()))?;
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let Some(outcome) = parse_callback(&request, expected_state) else {
                // Favicon or a stray request: answer and keep waiting.
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                continue;
            };
            let (title, body) = match &outcome {
                Ok(_) => ("Signed in", "You're signed in to OGS. You can close this tab and return to Tengen."),
                Err(_) => ("Sign-in didn't finish", "OGS didn't confirm the sign-in. Return to Tengen and try again."),
            };
            let page = callback_page(title, body);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = stream.write_all(reply.as_bytes()).await;
            return outcome;
        }
    })
    .await
    .map_err(|_| AdapterError::Protocol("sign-in timed out".into()))?
}

/// Reads `GET /oauth/callback?code=…&state=…`. `None` means "not the callback".
fn parse_callback(request: &str, expected_state: &str) -> Option<Result<String, AdapterError>> {
    let target = request.lines().next()?.split_whitespace().nth(1)?;
    if !target.starts_with("/oauth/callback") {
        return None;
    }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    let get = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());
    if get("state").as_deref() != Some(expected_state) {
        return Some(Err(AdapterError::Protocol("sign-in reply didn't match this request".into())));
    }
    if let Some(err) = get("error") {
        return Some(Err(if err == "access_denied" {
            AdapterError::BadCredentials
        } else {
            AdapterError::Protocol(get("error_description").unwrap_or(err))
        }));
    }
    Some(get("code").ok_or_else(|| AdapterError::Protocol("OGS sent no sign-in code".into())))
}

fn callback_page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title>\
         <style>body{{font:16px -apple-system,sans-serif;background:#dcdfda;color:#1f2422;display:grid;place-items:center;height:100vh;margin:0}}\
         h1{{font:500 22px 'Hiragino Mincho ProN',serif;margin:0 0 8px}}@media(prefers-color-scheme:dark){{body{{background:#161a19;color:#dfe2de}}}}</style>\
         <div><h1>{title}</h1><p>{body}</p></div>"
    )
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_the_hashed_verifier() {
        let p = Pkce::new();
        assert_eq!(p.challenge, URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes())));
        assert!(p.verifier.len() >= 43, "RFC 7636 asks for at least 43 characters");
    }

    #[test]
    fn authorize_url_carries_pkce_and_redirect() {
        let p = Pkce { verifier: "v".into(), challenge: "c h".into(), state: "s".into() };
        let url = authorize_url("https://online-go.com", "abc", &p);
        assert!(url.starts_with("https://online-go.com/oauth2/authorize/?client_id=abc&response_type=code"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A47613%2Foauth%2Fcallback"));
        assert!(url.contains("code_challenge=c+h&code_challenge_method=S256"));
    }

    #[test]
    fn reads_the_callback() {
        let ok = parse_callback("GET /oauth/callback?code=xyz&state=s HTTP/1.1\r\nHost: x\r\n\r\n", "s");
        assert_eq!(ok.unwrap().unwrap(), "xyz");
        assert!(parse_callback("GET /favicon.ico HTTP/1.1\r\n", "s").is_none());
        assert!(parse_callback("GET /oauth/callback?code=xyz&state=other HTTP/1.1\r\n", "s").unwrap().is_err());
        let denied = parse_callback("GET /oauth/callback?error=access_denied&state=s HTTP/1.1\r\n", "s");
        assert!(matches!(denied.unwrap(), Err(AdapterError::BadCredentials)));
    }

    #[tokio::test]
    async fn serves_the_callback_page_end_to_end() {
        let waiting = tokio::spawn(wait_for_code("st", std::time::Duration::from_secs(5)));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", CALLBACK_PORT)).await.unwrap();
        s.write_all(b"GET /oauth/callback?code=c0de&state=st HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await.unwrap();
        let mut page = String::new();
        s.read_to_string(&mut page).await.unwrap();
        assert!(page.contains("You're signed in"));
        assert_eq!(waiting.await.unwrap().unwrap(), "c0de");
    }
}
