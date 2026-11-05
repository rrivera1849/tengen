//! Adapter for the Fox Go Server (野狐围棋, foxwq.com).
//!
//! Fox has no public API; how it talks is written down in `PROTOCOL.md`.
//! Signing in takes two connections: the navigation server checks the
//! password and hands back a session token and the gateway's address; the
//! gateway then carries everything else, with a heartbeat every five seconds.
//!
//! - [`wire`]: just enough protobuf to read and write Fox's messages.
//! - [`frame`]: Fox's frames (length, type, checksum, optional zlib).
//! - [`protocol`]: Fox messages to Tengen events and back, without networking.
//!
//! Fox doesn't allow unofficial clients, so accounts used with Tengen could
//! be restricted. Tengen behaves like a person using the official client:
//! it never plays on its own.

pub mod frame;
pub mod protocol;
pub mod wire;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngCore;
use tengen_core::adapter::{AdapterError, Credentials, Speed};
use tengen_core::live::{LiveClient, LiveEvent};
use tengen_core::{Move, Point};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use frame::{Frame, GATEWAY_PACKET};
use protocol::{Out, Translator};

pub struct Fox {
    outbox: mpsc::UnboundedSender<Out>,
    translator: Arc<Mutex<Translator>>,
    closed: Arc<AtomicBool>,
}

/// Signs in and starts listening. Events (starting with `Ready`) go to `events`.
pub async fn connect(
    credentials: &Credentials,
    events: mpsc::UnboundedSender<LiveEvent>,
) -> Result<Fox, AdapterError> {
    connect_to(protocol::NAV_HOST, protocol::NAV_PORT, credentials, events).await
}

pub async fn connect_to(
    nav_host: &str,
    nav_port: u16,
    credentials: &Credentials,
    events: mpsc::UnboundedSender<LiveEvent>,
) -> Result<Fox, AdapterError> {
    // 1. The navigation server: password, then the gateway's address.
    let mut nav = open(nav_host, nav_port).await?;
    let mut buf = Vec::new();
    send_frame(
        &mut nav,
        protocol::NAV_LOGIN,
        &protocol::login_request(&credentials.username, &credentials.password, &mac_address()),
    )
    .await?;
    let reply = wait_for(&mut nav, &mut buf, protocol::NAV_LOGIN_REPLY).await?;
    let login = protocol::login_reply(&reply.payload)?;
    for (kind, body) in protocol::servers_request(login.uid) {
        send_frame(&mut nav, kind, &body).await?;
    }
    let list = wait_for(&mut nav, &mut buf, protocol::NAV_SERVERS_REPLY).await?;
    let (host, port) = protocol::gateway_address(&list.payload)
        .ok_or_else(|| AdapterError::Protocol("Fox sent no gateway address".into()))?;
    drop(nav);

    // 2. The gateway, for the rest of the session.
    let gateway = open(&host, port).await?;
    let (mut reader, mut writer) = gateway.into_split();
    let uid = login.uid;
    let translator = Arc::new(Mutex::new(Translator::new(login)));
    let closed = Arc::new(AtomicBool::new(false));
    let (outbox, mut queue) = mpsc::unbounded_channel::<Out>();
    let _ = outbox.send(translator.lock().expect("fox translator").gateway_login());

    tokio::spawn(async move {
        while let Some(out) = queue.recv().await {
            let bytes = frame::encode(GATEWAY_PACKET, &out.encode(uid));
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });

    let (beat_t, beat_out, beat_closed) =
        (Arc::clone(&translator), outbox.clone(), Arc::clone(&closed));
    tokio::spawn(async move {
        let start = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if beat_closed.load(Ordering::SeqCst) {
                return;
            }
            let beats = beat_t
                .lock()
                .expect("fox translator")
                .heartbeat(start.elapsed().as_millis() as u64);
            for b in beats {
                if beat_out.send(b).is_err() {
                    return;
                }
            }
        }
    });

    let (t, out, done) = (Arc::clone(&translator), outbox.clone(), Arc::clone(&closed));
    tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut chunk = vec![0u8; 16 * 1024];
        let reason = loop {
            match frame::take(&mut buf) {
                Ok(Some(f)) => {
                    if f.kind != GATEWAY_PACKET {
                        continue;
                    }
                    let result = t.lock().expect("fox translator").handle(&f.payload);
                    for s in result.send {
                        let _ = out.send(s);
                    }
                    let mut stop = false;
                    for e in result.events {
                        stop |= matches!(e, LiveEvent::Disconnected { .. });
                        let _ = events.send(e);
                    }
                    if stop {
                        done.store(true, Ordering::SeqCst);
                        return;
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => break format!("Fox sent something unreadable ({})", e.0),
            }
            match tokio::time::timeout(Duration::from_secs(30), reader.read(&mut chunk)).await {
                Ok(Ok(0)) | Ok(Err(_)) => break "Fox closed the connection".to_string(),
                Err(_) => break "Fox stopped answering".to_string(),
                Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        if !done.swap(true, Ordering::SeqCst) {
            let _ = events.send(LiveEvent::Disconnected {
                reason,
                retry: true,
            });
        }
    });

    Ok(Fox {
        outbox,
        translator,
        closed,
    })
}

async fn open(host: &str, port: u16) -> Result<TcpStream, AdapterError> {
    tokio::time::timeout(Duration::from_secs(15), TcpStream::connect((host, port)))
        .await
        .map_err(|_| AdapterError::Network("Fox didn't answer".into()))?
        .map_err(|e| AdapterError::Network(e.to_string()))
}

async fn send_frame(stream: &mut TcpStream, kind: u16, body: &[u8]) -> Result<(), AdapterError> {
    stream
        .write_all(&frame::encode(kind, body))
        .await
        .map_err(|e| AdapterError::Network(e.to_string()))
}

/// Reads frames until one of `kind` arrives (others are skipped).
async fn wait_for(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    kind: u16,
) -> Result<Frame, AdapterError> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        while let Some(f) = frame::take(buf).map_err(|e| AdapterError::Protocol(e.0))? {
            if f.kind == kind {
                return Ok(f);
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let n = tokio::time::timeout(left, stream.read(&mut chunk))
            .await
            .map_err(|_| AdapterError::Network("Fox took too long to answer".into()))?
            .map_err(|e| AdapterError::Network(e.to_string()))?;
        if n == 0 {
            return Err(AdapterError::Network(
                "Fox closed the connection while signing in".into(),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// A random hardware address, as Fox's clients send one.
fn mac_address() -> String {
    let mut b = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02X}")).collect()
}

impl Fox {
    fn send(&self, out: Option<Out>) {
        if let Some(o) = out {
            let _ = self.outbox.send(o);
        }
    }
}

impl LiveClient for Fox {
    fn find_match(&self, size: u8, speed: Speed) -> Result<(), String> {
        let out = self
            .translator
            .lock()
            .expect("fox translator")
            .automatch(size, speed)?;
        self.send(Some(out));
        Ok(())
    }

    fn cancel_match(&self) {
        let out = self
            .translator
            .lock()
            .expect("fox translator")
            .cancel_automatch();
        self.send(Some(out));
    }

    fn play(&self, game_id: u64, mv: Move) {
        let out = self
            .translator
            .lock()
            .expect("fox translator")
            .play(game_id, mv);
        self.send(out);
    }

    fn resign(&self, game_id: u64) {
        let out = self
            .translator
            .lock()
            .expect("fox translator")
            .resign(game_id);
        self.send(out);
    }

    /// Fox judges dead stones itself when counting; there's nothing to mark.
    fn toggle_dead(&self, _game_id: u64, _point: Point, _dead: bool) {}

    fn marks_dead_stones(&self) -> bool {
        false
    }

    fn accept_score(&self, game_id: u64) {
        let out = self
            .translator
            .lock()
            .expect("fox translator")
            .accept_score(game_id);
        self.send(out);
    }

    fn leave_game(&self, game_id: u64) {
        self.translator
            .lock()
            .expect("fox translator")
            .leave(game_id);
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use wire::{Msg, Pb};

    /// A stand-in for Fox: a navigation server and a gateway on localhost.
    #[tokio::test]
    async fn signs_in_through_navigation_and_gateway() {
        let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gw_port = gateway.local_addr().unwrap().port();
        let nav = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let nav_port = nav.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut s, _) = nav.accept().await.unwrap();
            let mut buf = Vec::new();
            let login = wait_for(&mut s, &mut buf, protocol::NAV_LOGIN)
                .await
                .unwrap();
            assert_eq!(Msg::decode(&login.payload).unwrap().text(1), "rafael");
            let reply = Pb::new()
                .int(1, 0)
                .int(2, 777)
                .msg(3, Pb::new().str(3, "rafael").int(4, 3))
                .bytes(8, b"tok")
                .build();
            send_frame(&mut s, protocol::NAV_LOGIN_REPLY, &reply)
                .await
                .unwrap();
            wait_for(&mut s, &mut buf, protocol::NAV_SERVERS_2)
                .await
                .unwrap();
            let list = Pb::new()
                .msg(
                    3,
                    Pb::new()
                        .int(1, 1)
                        .str(21, "127.0.0.1")
                        .int(22, u64::from(gw_port)),
                )
                .build();
            send_frame(&mut s, protocol::NAV_SERVERS_REPLY, &list)
                .await
                .unwrap();
        });

        tokio::spawn(async move {
            let (mut s, _) = gateway.accept().await.unwrap();
            let mut buf = Vec::new();
            let first = wait_for(&mut s, &mut buf, GATEWAY_PACKET).await.unwrap();
            let (head, body) = frame::split_gateway(&first.payload).unwrap();
            assert_eq!(
                Msg::decode(head).unwrap().u(7),
                u64::from(protocol::GATEWAY)
            );
            assert_eq!(Msg::decode(body).unwrap().bytes(1), Some(&b"tok"[..]));
            let ok = Out {
                server: protocol::GATEWAY,
                id: 1,
                server_id: 0,
                body: Pb::new().int(1, 0).build(),
            }
            .encode(777);
            send_frame(&mut s, GATEWAY_PACKET, &ok).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let (tx, mut rx) = mpsc::unbounded_channel();
        let creds = Credentials {
            username: "rafael".into(),
            password: "pw".into(),
        };
        let fox = connect_to("127.0.0.1", nav_port, &creds, tx).await.unwrap();
        let ready = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(ready, LiveEvent::Ready { account } if account.username == "rafael" && account.rank.as_deref() == Some("15k"))
        );
        fox.close();
    }

    #[tokio::test]
    async fn a_wrong_password_is_reported_as_such() {
        let nav = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = nav.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = nav.accept().await.unwrap();
            let mut buf = Vec::new();
            wait_for(&mut s, &mut buf, protocol::NAV_LOGIN)
                .await
                .unwrap();
            send_frame(
                &mut s,
                protocol::NAV_LOGIN_REPLY,
                &Pb::new().i32(1, 111_231).build(),
            )
            .await
            .unwrap();
        });
        let (tx, _rx) = mpsc::unbounded_channel();
        let creds = Credentials {
            username: "rafael".into(),
            password: "nope".into(),
        };
        assert!(matches!(
            connect_to("127.0.0.1", port, &creds, tx).await,
            Err(AdapterError::BadCredentials)
        ));
    }
}
