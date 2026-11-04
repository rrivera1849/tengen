//! Adapter for [Pandanet](https://pandanet-igs.com), the Internet Go Server.
//!
//! IGS is a text protocol over a plain TCP connection, the same one its
//! telnet interface uses. After signing in Tengen turns on client mode, in
//! which every line starts with a number saying what kind of line it is.
//!
//! - [`protocol`]: IGS lines to Tengen events and back, without networking.

pub mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tengen_core::adapter::{AdapterError, Credentials, Speed};
use tengen_core::live::{LiveClient, LiveEvent};
use tengen_core::{Move, Point};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use protocol::{send, Translator};

pub const HOST: &str = "igs.joyjoy.net";
pub const PORT: u16 = 6969;

pub struct Igs {
    outbox: mpsc::UnboundedSender<String>,
    translator: Arc<Mutex<Translator>>,
    closed: Arc<AtomicBool>,
}

/// Signs in and starts listening. Events (starting with `Ready`) go to `events`.
pub async fn connect(
    host: &str,
    port: u16,
    credentials: &Credentials,
    events: mpsc::UnboundedSender<LiveEvent>,
) -> Result<Igs, AdapterError> {
    let net = |e: std::io::Error| AdapterError::Network(e.to_string());
    let mut stream =
        tokio::time::timeout(Duration::from_secs(15), TcpStream::connect((host, port)))
            .await
            .map_err(|_| AdapterError::Network("Pandanet didn't answer".into()))?
            .map_err(net)?;
    let leftover = tokio::time::timeout(Duration::from_secs(25), sign_in(&mut stream, credentials))
        .await
        .map_err(|_| AdapterError::Network("Pandanet didn't finish signing in".into()))??;

    let (mut reader, mut writer) = stream.into_split();
    let (outbox, mut queue) = mpsc::unbounded_channel::<String>();
    let translator = Arc::new(Mutex::new(Translator::new(&credentials.username)));
    let closed = Arc::new(AtomicBool::new(false));

    // Client mode first, so every later line carries its type number.
    for cmd in [
        "toggle client true",
        "id Tengen",
        "toggle seek on",
        "seek config_list",
    ] {
        let _ = outbox.send(cmd.to_string());
    }
    let account = translator.lock().expect("igs translator").account();
    let _ = events.send(LiveEvent::Ready { account });

    tokio::spawn(async move {
        while let Some(cmd) = queue.recv().await {
            let bye = cmd == "exit";
            if writer
                .write_all(format!("{cmd}\r\n").as_bytes())
                .await
                .is_err()
                || bye
            {
                break;
            }
        }
    });

    // IGS drops connections that stay silent too long.
    let keepalive = outbox.clone();
    let alive = Arc::clone(&closed);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(240)).await;
            if alive.load(Ordering::SeqCst) || keepalive.send("ayt".into()).is_err() {
                return;
            }
        }
    });

    let (t, out, done) = (Arc::clone(&translator), outbox.clone(), Arc::clone(&closed));
    tokio::spawn(async move {
        let mut pending = leftover;
        let mut buf = vec![0u8; 8192];
        loop {
            while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                let result = t.lock().expect("igs translator").handle(&line);
                for c in result.commands {
                    let _ = out.send(c);
                }
                for e in result.events {
                    let _ = events.send(e);
                }
            }
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    if !done.swap(true, Ordering::SeqCst) {
                        let _ = events.send(LiveEvent::Disconnected {
                            reason: "Pandanet closed the connection".into(),
                            retry: true,
                        });
                    }
                    return;
                }
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
        }
    });

    Ok(Igs {
        outbox,
        translator,
        closed,
    })
}

/// Answers the `Login:` and `Password:` prompts. Returns whatever arrived
/// after the first command prompt, to be read as ordinary lines.
async fn sign_in(
    stream: &mut TcpStream,
    credentials: &Credentials,
) -> Result<Vec<u8>, AdapterError> {
    let net = |e: std::io::Error| AdapterError::Network(e.to_string());
    let mut seen = String::new();
    let mut buf = vec![0u8; 4096];
    let mut stage = Stage::Name;
    loop {
        let n = stream.read(&mut buf).await.map_err(net)?;
        if n == 0 {
            return Err(AdapterError::Network(
                "Pandanet closed the connection while signing in".into(),
            ));
        }
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        match stage::next(stage, &seen) {
            Step::Wait => {}
            Step::Send(next, what) => {
                let text = match what {
                    Field::Name => &credentials.username,
                    Field::Password => &credentials.password,
                };
                stream
                    .write_all(format!("{text}\r\n").as_bytes())
                    .await
                    .map_err(net)?;
                stage = next;
                seen.clear();
            }
            Step::Done(rest) => return Ok(rest.into_bytes()),
            Step::Fail(e) => return Err(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Name,
    Password,
    Prompt,
}

#[derive(Debug, PartialEq)]
enum Field {
    Name,
    Password,
}

#[derive(Debug)]
enum Step {
    Wait,
    Send(Stage, Field),
    Done(String),
    Fail(AdapterError),
}

mod stage {
    use super::*;

    /// What to do given everything received since the last thing sent.
    pub(super) fn next(stage: Stage, seen: &str) -> Step {
        let lower = seen.to_lowercase();
        match stage {
            Stage::Name if seen.contains("Login:") => Step::Send(Stage::Password, Field::Name),
            Stage::Password if seen.contains("Password:") || seen.contains("1 1") => {
                Step::Send(Stage::Prompt, Field::Password)
            }
            // An unregistered name is let in as a guest instead of asking for a password.
            Stage::Password | Stage::Prompt if lower.contains("guest") => {
                Step::Fail(AdapterError::Protocol(
                    "Pandanet doesn't know that account name. Register at pandanet-igs.com first."
                        .into(),
                ))
            }
            Stage::Prompt
                if lower.contains("invalid password")
                    || lower.contains("sorry")
                    || seen.contains("Login:") =>
            {
                Step::Fail(AdapterError::BadCredentials)
            }
            Stage::Prompt => {
                // Signed in once the command prompt (`#>`, or `1 5` in client mode) shows.
                for marker in ["#> ", "#>", "\n1 5", "\n1 6"] {
                    if let Some(i) = seen.find(marker) {
                        let after = &seen[i + marker.len()..];
                        return Step::Done(after.to_string());
                    }
                }
                Step::Wait
            }
            _ => Step::Wait,
        }
    }
}

impl LiveClient for Igs {
    fn find_match(&self, size: u8, speed: Speed) -> Result<(), String> {
        let cmd = self
            .translator
            .lock()
            .expect("igs translator")
            .automatch(size, speed)?;
        let _ = self.outbox.send(cmd);
        Ok(())
    }

    fn cancel_match(&self) {
        self.translator.lock().expect("igs translator").searching = false;
        let _ = self.outbox.send("seek entry_cancel".into());
    }

    fn play(&self, game_id: u64, mv: Move) {
        let size = self
            .translator
            .lock()
            .expect("igs translator")
            .game_size(game_id)
            .unwrap_or(19);
        let _ = self.outbox.send(send::play(size, game_id, mv));
    }

    fn resign(&self, _game_id: u64) {
        let _ = self.outbox.send("resign".into());
    }

    fn toggle_dead(&self, game_id: u64, point: Point, dead: bool) {
        let size = self
            .translator
            .lock()
            .expect("igs translator")
            .game_size(game_id)
            .unwrap_or(19);
        // IGS can only remove; bringing stones back resets every removal.
        let cmd = if dead {
            send::remove(size, game_id, point)
        } else {
            "undo".into()
        };
        let _ = self.outbox.send(cmd);
    }

    fn accept_score(&self, _game_id: u64) {
        let _ = self.outbox.send("done".into());
    }

    fn leave_game(&self, _game_id: u64) {}

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.outbox.send("exit".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_answers_each_prompt_in_turn() {
        assert!(matches!(
            stage::next(Stage::Name, "Welcome to IGS\r\nLogin: "),
            Step::Send(Stage::Password, Field::Name)
        ));
        assert!(matches!(
            stage::next(Stage::Password, "Password: "),
            Step::Send(Stage::Prompt, Field::Password)
        ));
        assert!(
            matches!(stage::next(Stage::Prompt, "Last login: today\r\n#> 9 hello\n"), Step::Done(rest) if rest == "9 hello\n")
        );
        assert!(matches!(
            stage::next(Stage::Prompt, "Last login"),
            Step::Wait
        ));
    }

    #[test]
    fn sign_in_failures_are_named() {
        assert!(matches!(
            stage::next(Stage::Prompt, "Invalid password.\r\n"),
            Step::Fail(AdapterError::BadCredentials)
        ));
        assert!(matches!(
            stage::next(Stage::Password, "This is a guest account.\r\n"),
            Step::Fail(AdapterError::Protocol(_))
        ));
    }
}
