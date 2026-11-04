//! Adapter for [KGS](https://www.gokgs.com), through its official JSON API.
//!
//! KGS keeps a session cookie. Tengen POSTs each message it sends and keeps
//! one GET open at a time to receive messages (long polling): a GET returns
//! as soon as KGS has something to say, or empty after a minute.
//!
//! - [`protocol`]: KGS messages to Tengen events and back, without networking.

pub mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tengen_core::adapter::{AdapterError, Credentials, Speed};
use tengen_core::live::{LiveClient, LiveEvent};
use tengen_core::{Move, Point};
use tokio::sync::mpsc;

use protocol::{send, Translator};

pub const ENDPOINT: &str = "https://www.gokgs.com/json/access";

/// A signed-in KGS session. Dropping it doesn't sign out; call [`LiveClient::close`].
pub struct Kgs {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    url: String,
    outbox: mpsc::UnboundedSender<Value>,
    translator: Mutex<Translator>,
    closed: AtomicBool,
}

/// Signs in and starts listening. Events (starting with `Ready`) go to `events`.
pub async fn connect(
    url: &str,
    credentials: &Credentials,
    events: mpsc::UnboundedSender<LiveEvent>,
) -> Result<Kgs, AdapterError> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("Tengen/", env!("CARGO_PKG_VERSION")))
        .cookie_store(true)
        // A GET may wait a full minute for news before KGS answers.
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| AdapterError::Network(e.to_string()))?;
    post(
        &http,
        url,
        &send::login(&credentials.username, &credentials.password),
    )
    .await?;

    let mut translator = Translator::new();
    let mut first = None;
    // The answer to LOGIN arrives on the next GETs, usually the first.
    for _ in 0..5 {
        let batch = receive(&http, url).await?;
        if let Some(failure) = batch.iter().find_map(login_failure) {
            return Err(failure);
        }
        if batch.iter().any(|m| m["type"] == "LOGIN_SUCCESS") {
            first = Some(batch);
            break;
        }
        if batch.iter().any(|m| m["type"] == "LOGOUT") {
            return Err(AdapterError::Protocol(
                "KGS closed the connection while signing in".into(),
            ));
        }
    }
    let first =
        first.ok_or_else(|| AdapterError::Network("KGS didn't answer the sign-in".into()))?;
    let out = translator.handle(&first);

    let (outbox, mut queue) = mpsc::unbounded_channel::<Value>();
    let inner = Arc::new(Inner {
        http,
        url: url.to_string(),
        outbox,
        translator: Mutex::new(translator),
        closed: AtomicBool::new(false),
    });
    for e in out.events {
        let _ = events.send(e);
    }
    for r in out.replies {
        let _ = inner.outbox.send(r);
    }

    // Sends go out one at a time, in order.
    let sender = Arc::clone(&inner);
    tokio::spawn(async move {
        while let Some(msg) = queue.recv().await {
            let logout = msg["type"] == "LOGOUT";
            // A failed send means the session is gone; the poller notices
            // and reports the disconnect, so there's nothing more to do here.
            if let Err(e) = post(&sender.http, &sender.url, &msg).await {
                if !sender.closed.load(Ordering::SeqCst) {
                    eprintln!("KGS send failed: {e}");
                }
            }
            if logout {
                break;
            }
        }
    });

    let poller = Arc::clone(&inner);
    tokio::spawn(async move { poll(poller, events).await });
    Ok(Kgs { inner })
}

async fn poll(inner: Arc<Inner>, events: mpsc::UnboundedSender<LiveEvent>) {
    let mut failures = 0;
    loop {
        if inner.closed.load(Ordering::SeqCst) {
            return;
        }
        match receive(&inner.http, &inner.url).await {
            Ok(batch) => {
                failures = 0;
                let out = inner
                    .translator
                    .lock()
                    .expect("kgs translator")
                    .handle(&batch);
                for r in out.replies {
                    let _ = inner.outbox.send(r);
                }
                let mut stop = batch.iter().any(|m| m["type"] == "LOGOUT");
                for e in out.events {
                    stop |= matches!(e, LiveEvent::Disconnected { .. });
                    let _ = events.send(e);
                }
                if stop {
                    inner.closed.store(true, Ordering::SeqCst);
                    return;
                }
            }
            Err(e) => {
                failures += 1;
                if failures > 3 {
                    if !inner.closed.swap(true, Ordering::SeqCst) {
                        let _ = events.send(LiveEvent::Disconnected {
                            reason: e.to_string(),
                            retry: true,
                        });
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

fn login_failure(m: &Value) -> Option<AdapterError> {
    match m["type"].as_str()? {
        "LOGIN_FAILED_NO_SUCH_USER" | "LOGIN_FAILED_BAD_PASSWORD" => {
            Some(AdapterError::BadCredentials)
        }
        "LOGIN_FAILED_USER_ALREADY_EXISTS" => {
            Some(AdapterError::Protocol("that name is already in use".into()))
        }
        "LOGIN_FAILED_KEEP_OUT" => Some(AdapterError::Protocol(format!(
            "KGS has blocked this account: {}",
            m["text"].as_str().unwrap_or("no reason given")
        ))),
        t if t.starts_with("LOGIN_FAILED") => {
            Some(AdapterError::Protocol(t.to_lowercase().replace('_', " ")))
        }
        _ => None,
    }
}

async fn post(http: &reqwest::Client, url: &str, msg: &Value) -> Result<(), AdapterError> {
    if msg.is_null() {
        return Ok(());
    }
    let resp = http
        .post(url)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(ascii_json(msg))
        .send()
        .await
        .map_err(|e| AdapterError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(AdapterError::Network(format!(
            "KGS answered {}",
            resp.status()
        )));
    }
    Ok(())
}

async fn receive(http: &reqwest::Client, url: &str) -> Result<Vec<Value>, AdapterError> {
    let resp = http
        .get(url)
        .header("Accept", "application/json;charset=UTF-8")
        .send()
        .await
        .map_err(|e| AdapterError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(AdapterError::Network(format!(
            "KGS answered {}",
            resp.status()
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| AdapterError::Protocol(e.to_string()))?;
    Ok(body["messages"].as_array().cloned().unwrap_or_default())
}

/// JSON with every non-ASCII character escaped, as KGS's own clients send it.
fn ascii_json(v: &Value) -> String {
    let mut out = String::new();
    for c in v.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

impl Kgs {
    fn send(&self, msg: Value) {
        let _ = self.inner.outbox.send(msg);
    }
}

impl LiveClient for Kgs {
    fn find_match(&self, size: u8, speed: Speed) -> Result<(), String> {
        let msg = self
            .inner
            .translator
            .lock()
            .expect("kgs translator")
            .automatch(size, speed)?;
        self.send(msg);
        Ok(())
    }

    fn cancel_match(&self) {
        self.inner
            .translator
            .lock()
            .expect("kgs translator")
            .searching = false;
        self.send(send::automatch_cancel());
    }

    fn play(&self, game_id: u64, mv: Move) {
        self.send(send::play(game_id, mv));
    }

    fn resign(&self, game_id: u64) {
        self.send(send::resign(game_id));
    }

    fn toggle_dead(&self, game_id: u64, point: Point, dead: bool) {
        self.send(send::mark_life(game_id, point, !dead));
    }

    fn accept_score(&self, game_id: u64) {
        let done = self
            .inner
            .translator
            .lock()
            .expect("kgs translator")
            .done_id(game_id);
        if let Some(done) = done {
            self.send(send::scoring_done(game_id, done));
        }
    }

    fn leave_game(&self, game_id: u64) {
        self.send(send::leave(game_id));
    }

    fn close(&self) {
        self.inner
            .translator
            .lock()
            .expect("kgs translator")
            .logging_out = true;
        self.send(send::logout());
        self.inner.closed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn non_ascii_is_escaped() {
        let expected = format!("{{\"text\":\"h{0}u00e9llo {0}u56f2{0}u7881\"}}", '\\');
        assert_eq!(
            ascii_json(&json!({ "text": "h\u{e9}llo \u{56f2}\u{7881}" })),
            expected
        );
        let back: Value = serde_json::from_str(&ascii_json(&json!({ "t": "\u{1d11e}" }))).unwrap();
        assert_eq!(back["t"], "\u{1d11e}");
    }

    #[test]
    fn login_failures_are_named() {
        assert!(matches!(
            login_failure(&json!({ "type": "LOGIN_FAILED_BAD_PASSWORD" })),
            Some(AdapterError::BadCredentials)
        ));
        assert!(login_failure(&json!({ "type": "HELLO" })).is_none());
    }
}
