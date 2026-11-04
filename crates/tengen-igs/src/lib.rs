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
    let mut transcript = String::new();
    let leftover = tokio::time::timeout(
        Duration::from_secs(25),
        sign_in(&mut stream, credentials, &mut transcript),
    )
    .await
    .map_err(|_| {
        AdapterError::Network(format!(
            "Pandanet didn't finish signing in{}",
            last_words(&transcript, &credentials.password)
        ))
    })??;

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
                .write_all(format!("{cmd}\n").as_bytes())
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
                let line = String::from_utf8_lossy(&strip_telnet(&line)).into_owned();
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
/// Everything received is also added to `transcript`, so a failure can say
/// what the server said last.
async fn sign_in(
    stream: &mut TcpStream,
    credentials: &Credentials,
    transcript: &mut String,
) -> Result<Vec<u8>, AdapterError> {
    let net = |e: std::io::Error| AdapterError::Network(e.to_string());
    // Raw bytes since the last thing sent; telnet commands can split across reads.
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 4096];
    let mut stage = Stage::Name;
    loop {
        let n = stream.read(&mut buf).await.map_err(net)?;
        if n == 0 {
            return Err(AdapterError::Network(format!(
                "Pandanet closed the connection while signing in{}",
                last_words(transcript, &credentials.password)
            )));
        }
        transcript.push_str(&String::from_utf8_lossy(&buf[..n]));
        raw.extend_from_slice(&buf[..n]);
        let seen = String::from_utf8_lossy(&strip_telnet(&raw)).into_owned();
        match stage::next(stage, &seen) {
            Step::Wait => {}
            Step::Send(next, what) => {
                let text = match what {
                    Field::Name => &credentials.username,
                    Field::Password => &credentials.password,
                };
                stream
                    .write_all(format!("{text}\n").as_bytes())
                    .await
                    .map_err(net)?;
                stage = next;
                raw.clear();
            }
            Step::Done(rest) => return Ok(rest.into_bytes()),
            Step::Fail(e) => return Err(e),
        }
    }
}

/// Removes telnet commands (`IAC` sequences such as "won't echo", sent around
/// the password prompt) so they can't hide the text next to them.
fn strip_telnet(bytes: &[u8]) -> Vec<u8> {
    const IAC: u8 = 255;
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != IAC {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            Some(&IAC) => {
                out.push(IAC); // an escaped 255
                i += 2;
            }
            // WILL, WONT, DO, DONT carry one option byte.
            Some(251..=254) => i += 3,
            _ => i += 2,
        }
    }
    out
}

/// The last couple of lines the server sent, for an error message. Telnet
/// control bytes are dropped and the password never appears.
fn last_words(transcript: &str, password: &str) -> String {
    let clean: String = transcript
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .filter(|c| *c != '\u{fffd}')
        .collect();
    let mut clean = clean;
    if !password.is_empty() {
        clean = clean.replace(password, "•••");
    }
    let lines: Vec<&str> = clean
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return " before saying anything".into();
    }
    let tail = lines[lines.len().saturating_sub(2)..].join(" / ");
    let tail: String = tail.chars().take(200).collect();
    format!(". It last said: \u{201c}{tail}\u{201d}")
}

/// Signs in, waits a few seconds, and returns everything the server sent.
/// For diagnosing sign-in problems (`cargo run -p tengen-igs --example igs_login`).
pub async fn sign_in_transcript(
    host: &str,
    port: u16,
    credentials: &Credentials,
) -> (Result<(), AdapterError>, String) {
    let mut transcript = String::new();
    let mut stream = match TcpStream::connect((host, port)).await {
        Ok(s) => s,
        Err(e) => return (Err(AdapterError::Network(e.to_string())), transcript),
    };
    let result = tokio::time::timeout(
        Duration::from_secs(25),
        sign_in(&mut stream, credentials, &mut transcript),
    )
    .await;
    let result = match result {
        Err(_) => Err(AdapterError::Network("timed out".into())),
        Ok(r) => r.map(|_| ()),
    };
    if result.is_ok() {
        let _ = stream.write_all(b"toggle client true\n").await;
        let mut buf = vec![0u8; 8192];
        let until = tokio::time::Instant::now() + Duration::from_secs(4);
        while let Ok(Ok(n)) = tokio::time::timeout_at(until, stream.read(&mut buf)).await {
            if n == 0 {
                break;
            }
            transcript.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        let _ = stream.write_all(b"exit\n").await;
    }
    (result, transcript.replace(&credentials.password, "•••"))
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
        match stage {
            Stage::Name if seen.contains("Login:") => Step::Send(Stage::Password, Field::Name),
            Stage::Password if seen.contains("Password:") || has_line(seen, "1 1") => {
                Step::Send(Stage::Prompt, Field::Password)
            }
            Stage::Password if unknown_name(seen) => Step::Fail(not_registered()),
            Stage::Prompt => {
                // The prompt means we're in, whatever the welcome text said before it.
                if let Some(rest) = after_prompt(seen) {
                    return Step::Done(rest);
                }
                if unknown_name(seen) {
                    return Step::Fail(not_registered());
                }
                let lower = seen.to_lowercase();
                if lower.contains("invalid password")
                    || lower.contains("incorrect password")
                    || seen.contains("Login:")
                {
                    return Step::Fail(AdapterError::BadCredentials);
                }
                Step::Wait
            }
            _ => Step::Wait,
        }
    }

    /// The text after the command prompt: `#>`, or in client mode (which an
    /// account can keep switched on) a line like `1 5`.
    fn after_prompt(seen: &str) -> Option<String> {
        if let Some(i) = seen.find("#>") {
            return Some(seen[i + 2..].trim_start_matches(' ').to_string());
        }
        let mut at = 0;
        for line in seen.split_inclusive('\n') {
            at += line.len();
            let t = line.trim();
            if t.len() == 3 && t.starts_with("1 ") && matches!(t.as_bytes()[2], b'5'..=b'8') {
                return Some(seen[at..].to_string());
            }
        }
        None
    }

    fn has_line(seen: &str, wanted: &str) -> bool {
        seen.lines().any(|l| l.trim() == wanted)
    }

    /// An unregistered name is let in as a guest instead of asked for a password.
    fn unknown_name(seen: &str) -> bool {
        let lower = seen.to_lowercase();
        lower.contains("guest account") || lower.contains("as a guest")
    }

    fn not_registered() -> AdapterError {
        AdapterError::Protocol(
            "Pandanet doesn't know that account name. Register at pandanet-igs.com first.".into(),
        )
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
    fn the_client_mode_prompt_counts_even_after_telnet_bytes() {
        // What Pandanet actually sent after the password (account with client mode on).
        let mut raw = b"9 File\r\n39 IGS entry on 11 - 05 - 2026\r\n".to_vec();
        raw.extend_from_slice(&[255, 252, 1]);
        raw.extend_from_slice(b"1 5\r\n9 hello\r\n");
        let seen = String::from_utf8_lossy(&strip_telnet(&raw)).into_owned();
        assert!(
            matches!(stage::next(Stage::Prompt, &seen), Step::Done(rest) if rest == "9 hello\r\n")
        );
        // A prompt at the very start, with nothing before it.
        assert!(matches!(stage::next(Stage::Prompt, "1 5\n"), Step::Done(rest) if rest.is_empty()));
        // `1 1` is the password prompt, not "signed in".
        assert!(matches!(stage::next(Stage::Prompt, "1 1\n"), Step::Wait));
        // Welcome text that happens to say "sorry" or "guest" isn't a failure.
        assert!(matches!(
            stage::next(
                Stage::Prompt,
                "Sorry for the downtime! Guests welcome.\r\n1 5\r\n"
            ),
            Step::Done(_)
        ));
    }

    #[test]
    fn telnet_commands_are_removed() {
        assert_eq!(
            strip_telnet(&[b'a', 255, 251, 1, b'b', 255, 255, b'c']),
            vec![b'a', b'b', 255, b'c']
        );
    }

    #[test]
    fn errors_quote_the_server_without_the_password() {
        let said = "Welcome\r\n\u{ff}\u{fb}\u{1}Password: \r\nSorry, hunter2 is wrong\r\n";
        let words = last_words(said, "hunter2");
        assert!(words.contains("Sorry, ••• is wrong"), "{words}");
        assert!(!words.contains("hunter2"));
        assert_eq!(last_words("", "x"), " before saying anything");
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
