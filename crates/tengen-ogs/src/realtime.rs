//! Live play over the OGS realtime websocket.
//!
//! Frames are JSON arrays. Tengen sends `[command, data]` or
//! `[command, data, id]`; OGS answers with `[event, data]`, or `[id, data]`
//! for a request that carried an id. Message names and fields follow OGS's
//! own `goban` library (src/engine/protocol).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tengen_core::adapter::{AdapterError, Speed};
use tengen_core::{Color, Game, GameInfo, Move, Point};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::rank_name;

/// The request id used for `authenticate`, so its reply can be recognised.
const AUTH_REQUEST: u64 = 1;
const PING_EVERY: Duration = Duration::from_secs(10);

/// What the OGS connection reports to the app.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum OgsEvent {
    Authenticated { user_id: u64, username: String },
    MatchStarted { uuid: String, game_id: u64 },
    MatchCanceled { uuid: Option<String> },
    GameData { game: Box<GameData> },
    Moved { game_id: u64, move_number: usize, point: Option<Point> },
    Clock { clock: ClockState },
    Phase { game_id: u64, phase: String },
    RemovedStones { game_id: u64, all_removed: String },
    ScoreAccepted { game_id: u64, player_id: u64, phase: String, outcome: Option<String>, winner: Option<u64> },
    GameError { game_id: u64, message: String },
    Error { message: String },
    Disconnected { reason: String },
}

/// The full state of a game as OGS sends it (`game/<id>/gamedata`).
/// Only the fields Tengen uses are read; OGS sends many more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameData {
    pub game_id: u64,
    pub width: u8,
    #[serde(default)]
    pub height: Option<u8>,
    #[serde(default)]
    pub komi: Option<f32>,
    pub players: Players,
    #[serde(default)]
    pub initial_player: Option<String>,
    #[serde(default)]
    pub initial_state: Option<InitialState>,
    #[serde(default)]
    pub moves: Vec<Value>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub winner: Option<Value>,
    /// Unix seconds.
    #[serde(default)]
    pub start_time: Option<i64>,
    #[serde(default)]
    pub removed: Option<Value>,
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub handicap: Option<u32>,
    #[serde(default)]
    pub free_handicap_placement: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Players {
    pub black: PlayerEntry,
    pub white: PlayerEntry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerEntry {
    pub id: u64,
    pub username: String,
    #[serde(default)]
    pub rank: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitialState {
    #[serde(default)]
    pub black: String,
    #[serde(default)]
    pub white: String,
}

impl GameData {
    pub fn color_of(&self, player_id: u64) -> Option<Color> {
        if self.players.black.id == player_id {
            Some(Color::Black)
        } else if self.players.white.id == player_id {
            Some(Color::White)
        } else {
            None
        }
    }

    pub fn is_finished(&self) -> bool {
        self.phase.as_deref() == Some("finished")
    }

    /// Who won, as a color, when the game is over.
    pub fn winner_color(&self) -> Option<Color> {
        match self.winner.as_ref()? {
            Value::String(s) if s == "black" => Some(Color::Black),
            Value::String(s) if s == "white" => Some(Color::White),
            Value::Number(n) => self.color_of(n.as_u64()?),
            _ => None,
        }
    }

    /// The SGF-style result, like `B+R` or `W+6.5`.
    pub fn result(&self) -> Option<String> {
        if !self.is_finished() {
            return None;
        }
        let winner = match self.winner_color()? {
            Color::Black => "B",
            Color::White => "W",
        };
        let outcome = self.outcome.as_deref().unwrap_or("");
        let short = match outcome {
            "Resignation" => "R".to_string(),
            "Timeout" => "T".to_string(),
            o => o.trim_end_matches(" points").to_string(),
        };
        Some(format!("{winner}+{short}"))
    }

    /// Builds a Tengen game record from OGS's state.
    pub fn to_game(&self) -> Game {
        let size = self.width;
        let (date, time) = match self.start_time {
            Some(t) => local_date_time_from_unix(t),
            None => (None, None),
        };
        let setup = |s: &str| decode_points(s, size);
        let info = GameInfo {
            size,
            komi: self.komi.unwrap_or(6.5),
            black: Some(self.players.black.username.clone()),
            white: Some(self.players.white.username.clone()),
            black_rank: self.players.black.rank.map(rank_name),
            white_rank: self.players.white.rank.map(rank_name),
            result: self.result(),
            date,
            time,
            rules: self.rules.clone(),
            server: Some("OGS".into()),
            event: None,
            setup_black: self.initial_state.as_ref().map(|s| setup(&s.black)).unwrap_or_default(),
            setup_white: self.initial_state.as_ref().map(|s| setup(&s.white)).unwrap_or_default(),
        };
        let mut game = Game::from_info(info);
        let first = if self.initial_player.as_deref() == Some("white") { Color::White } else { Color::Black };
        // With free handicap placement black's handicap stones arrive as moves.
        let handicap_moves = match (self.handicap, self.free_handicap_placement) {
            (Some(h), Some(true)) if h > 1 => h as usize,
            _ => 0,
        };
        let mut next = first;
        for (i, mv) in self.moves.iter().enumerate() {
            let Some((point, explicit)) = packed_move(mv, size) else { continue };
            let color = explicit.unwrap_or(if i < handicap_moves { Color::Black } else { next });
            game.play_record(color, point.map_or(Move::Pass, |p| Move::Play { point: p }));
            next = if i + 1 < handicap_moves { Color::Black } else { color.opposite() };
        }
        game
    }
}

/// Each player's clock, already adjusted to the moment it was received.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockState {
    pub game_id: u64,
    pub to_move: Option<Color>,
    pub black: PlayerClock,
    pub white: PlayerClock,
    pub paused: bool,
}

/// Seconds left, as of when the clock message arrived.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerClock {
    pub main: f64,
    pub periods: Option<u32>,
    pub period_time: Option<f64>,
    /// Time left in the current byo-yomi period, once main time is used up.
    pub period_left: Option<f64>,
}

#[derive(Deserialize)]
struct RawClock {
    game_id: u64,
    current_player: Option<u64>,
    black_player_id: Option<u64>,
    white_player_id: Option<u64>,
    #[serde(default)]
    black_time: Option<Value>,
    #[serde(default)]
    white_time: Option<Value>,
    /// Milliseconds since 1970.
    last_move: Option<f64>,
    now: Option<f64>,
    #[serde(default)]
    pause: Option<Value>,
    paused_since: Option<f64>,
}

impl RawClock {
    fn into_state(self) -> ClockState {
        let to_move = match self.current_player {
            Some(p) if Some(p) == self.black_player_id => Some(Color::Black),
            Some(p) if Some(p) == self.white_player_id => Some(Color::White),
            _ => None,
        };
        let paused = self.paused_since.is_some()
            || self.pause.as_ref().and_then(|p| p.get("paused")).and_then(Value::as_bool).unwrap_or(false);
        // The times are as of the last move; take off what has elapsed since,
        // for the player whose turn it is.
        let elapsed = match (self.now, self.last_move) {
            (Some(now), Some(last)) if !paused && now > last => (now - last) / 1000.0,
            _ => 0.0,
        };
        let read = |v: &Option<Value>, running: bool| {
            let mut c = parse_clock_time(v.as_ref());
            if running {
                c.spend(elapsed);
            }
            c
        };
        ClockState {
            game_id: self.game_id,
            to_move,
            black: read(&self.black_time, to_move == Some(Color::Black)),
            white: read(&self.white_time, to_move == Some(Color::White)),
            paused,
        }
    }
}

impl PlayerClock {
    /// Spends `secs` of thinking time, then byo-yomi periods.
    pub fn spend(&mut self, mut secs: f64) {
        let from_main = secs.min(self.main);
        self.main -= from_main;
        secs -= from_main;
        if secs <= 0.0 {
            return;
        }
        if let (Some(periods), Some(period)) = (self.periods, self.period_time) {
            let mut left = self.period_left.unwrap_or(period);
            let mut periods = periods;
            while secs > 0.0 && periods > 0 {
                if secs < left {
                    left -= secs;
                    secs = 0.0;
                } else {
                    secs -= left;
                    periods -= 1;
                    left = period;
                }
            }
            self.periods = Some(periods);
            self.period_left = Some(if periods == 0 { 0.0 } else { left });
        }
    }
}

fn parse_clock_time(v: Option<&Value>) -> PlayerClock {
    match v {
        Some(Value::Object(o)) => {
            let num = |k: &str| o.get(k).and_then(Value::as_f64);
            PlayerClock {
                main: num("thinking_time").unwrap_or(0.0).max(0.0),
                periods: num("periods").map(|p| p as u32),
                period_time: num("period_time"),
                period_left: num("period_time_left"),
            }
        }
        // Simple clocks send a bare number; Tengen shows no clock for those.
        _ => PlayerClock::default(),
    }
}

/// Reads one frame from OGS. Unknown or uninteresting frames give `None`.
pub fn parse_frame(text: &str) -> Option<OgsEvent> {
    let frame: Vec<Value> = serde_json::from_str(text).ok()?;
    let head = frame.first()?;
    let data = frame.get(1).cloned().unwrap_or(Value::Null);

    // A reply to one of our requests.
    if let Some(id) = head.as_u64() {
        if id != AUTH_REQUEST {
            return None;
        }
        if let Some(err) = frame.get(2).filter(|e| !e.is_null()) {
            let message = err.get("message").and_then(Value::as_str).unwrap_or("sign-in to the live server failed");
            return Some(OgsEvent::Error { message: message.to_string() });
        }
        return Some(OgsEvent::Authenticated {
            user_id: data.get("id")?.as_u64()?,
            username: data.get("username")?.as_str()?.to_string(),
        });
    }

    let name = head.as_str()?;
    match name {
        "automatch/start" => Some(OgsEvent::MatchStarted {
            uuid: data.get("uuid")?.as_str()?.to_string(),
            game_id: data.get("game_id")?.as_u64()?,
        }),
        "automatch/cancel" => Some(OgsEvent::MatchCanceled {
            uuid: data.get("uuid").and_then(Value::as_str).map(str::to_string),
        }),
        _ => {
            let rest = name.strip_prefix("game/")?;
            let (id, event) = rest.split_once('/')?;
            let game_id: u64 = id.parse().ok()?;
            match event {
                "gamedata" => serde_json::from_value::<GameData>(data)
                    .ok()
                    .map(|mut g| {
                        g.game_id = game_id;
                        OgsEvent::GameData { game: Box::new(g) }
                    }),
                "move" => {
                    let (point, _) = packed_move(data.get("move")?, 25)?;
                    Some(OgsEvent::Moved {
                        game_id,
                        move_number: data.get("move_number").and_then(Value::as_u64).unwrap_or(0) as usize,
                        point,
                    })
                }
                "clock" => serde_json::from_value::<RawClock>(data)
                    .ok()
                    .map(|c| OgsEvent::Clock { clock: c.into_state() }),
                "phase" => Some(OgsEvent::Phase { game_id, phase: data.as_str()?.to_string() }),
                "removed_stones" => Some(OgsEvent::RemovedStones {
                    game_id,
                    all_removed: data.get("all_removed")?.as_str()?.to_string(),
                }),
                "removed_stones_accepted" => Some(OgsEvent::ScoreAccepted {
                    game_id,
                    player_id: data.get("player_id").and_then(Value::as_u64).unwrap_or(0),
                    phase: data.get("phase").and_then(Value::as_str).unwrap_or("").to_string(),
                    outcome: data.get("outcome").and_then(Value::as_str).map(str::to_string),
                    winner: data.get("winner").and_then(Value::as_u64),
                }),
                "error" => Some(OgsEvent::GameError {
                    game_id,
                    message: data.as_str().unwrap_or("OGS rejected that").to_string(),
                }),
                _ => None,
            }
        }
    }
}

/// A packed move is `[x, y, time_delta?, color?, extra?]`; `x == -1` is a pass.
fn packed_move(v: &Value, size: u8) -> Option<(Option<Point>, Option<Color>)> {
    let a = v.as_array()?;
    let x = a.first()?.as_i64()?;
    let y = a.get(1)?.as_i64()?;
    let color = match a.get(3).and_then(Value::as_u64) {
        Some(1) => Some(Color::Black),
        Some(2) => Some(Color::White),
        _ => None,
    };
    if x < 0 || y < 0 {
        return Some((None, color));
    }
    if x >= size as i64 || y >= size as i64 {
        return None;
    }
    Some((Some(Point::new(x as u8, y as u8)), color))
}

/// OGS's move strings: two letters per point, `..` for a pass.
pub fn encode_move(point: Option<Point>) -> String {
    match point {
        Some(p) => format!("{}{}", (b'a' + p.x) as char, (b'a' + p.y) as char),
        None => "..".into(),
    }
}

/// Decodes a packed string of points such as `pddp`.
pub fn decode_points(s: &str, size: u8) -> Vec<Point> {
    s.as_bytes()
        .chunks(2)
        .filter_map(|c| match c {
            [x, y] if x.is_ascii_lowercase() && y.is_ascii_lowercase() => {
                let (x, y) = (x - b'a', y - b'a');
                (x < size && y < size).then(|| Point::new(x, y))
            }
            _ => None,
        })
        .collect()
}

/// Unix seconds to the local `YYYY-MM-DD` and `HH:MM`.
pub fn local_date_time_from_unix(secs: i64) -> (Option<String>, Option<String>) {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(secs, 0).single() {
        Some(t) => (Some(t.format("%Y-%m-%d").to_string()), Some(t.format("%H:%M").to_string())),
        None => (None, None),
    }
}

/// An RFC 3339 time (as in the REST API's `started`) to local date and time.
pub fn local_date_time_from_rfc3339(s: &str) -> (Option<String>, Option<String>) {
    match chrono::DateTime::parse_from_rfc3339(s) {
        Ok(t) => local_date_time_from_unix(t.timestamp()),
        Err(_) => (None, None),
    }
}

/// A live connection. Cheap to clone; dropping every clone closes the socket.
#[derive(Clone)]
pub struct Realtime {
    out: mpsc::UnboundedSender<String>,
}

impl Realtime {
    /// Opens the websocket, authenticates with the JWT from `/api/v1/ui/config`,
    /// and forwards everything OGS says to `events`.
    pub async fn connect(base: &str, jwt: &str, events: mpsc::UnboundedSender<OgsEvent>) -> Result<Realtime, AdapterError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let url = base.replacen("http", "ws", 1);
        let (socket, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|e| AdapterError::Network(format!("couldn't reach OGS live play: {e}")))?;
        let (mut sink, mut stream) = socket.split();
        let (out, mut outbox) = mpsc::unbounded_channel::<String>();

        // Writer: everything queued by the app, plus a ping every 10 seconds.
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            loop {
                let frame = tokio::select! {
                    next = outbox.recv() => match next {
                        Some(frame) => frame,
                        None => break,
                    },
                    _ = ping.tick() => json!(["net/ping", {"client": now_ms(), "drift": 0, "latency": 0}]).to_string(),
                };
                if sink.send(Message::text(frame)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        // Reader: OGS frames become events for the app.
        let to_app = events.clone();
        tokio::spawn(async move {
            let reason = loop {
                match stream.next().await {
                    Some(Ok(msg)) => {
                        if msg.is_close() {
                            break "OGS closed the connection".to_string();
                        }
                        if let Ok(text) = msg.to_text() {
                            if let Some(event) = parse_frame(text) {
                                if to_app.send(event).is_err() {
                                    break "Tengen stopped listening".to_string();
                                }
                            }
                        }
                    }
                    Some(Err(e)) => break format!("connection lost: {e}"),
                    None => break "connection lost".to_string(),
                }
            };
            let _ = to_app.send(OgsEvent::Disconnected { reason });
        });

        let rt = Realtime { out };
        rt.send(json!([
            "authenticate",
            {
                "jwt": jwt,
                "client": "tengen",
                "client_version": env!("CARGO_PKG_VERSION"),
                "user_agent": concat!("Tengen/", env!("CARGO_PKG_VERSION")),
                "language": "en",
            },
            AUTH_REQUEST
        ]));
        Ok(rt)
    }

    fn send(&self, frame: Value) {
        let _ = self.out.send(frame.to_string());
    }

    /// Asks OGS for an opponent. Returns the request's id, for cancelling.
    pub fn find_match(&self, size: u8, speed: Speed) -> String {
        let uuid = new_uuid();
        let speed = match speed {
            Speed::Blitz => "blitz",
            Speed::Rapid => "rapid",
            Speed::Live => "live",
            Speed::Correspondence => "correspondence",
        };
        self.send(json!(["automatch/find_match", {
            "uuid": uuid,
            "size_speed_options": [{ "size": format!("{size}x{size}"), "speed": speed, "system": "byoyomi" }],
            "lower_rank_diff": 3,
            "upper_rank_diff": 3,
            "rules": { "condition": "no-preference", "value": "japanese" },
            "handicap": { "condition": "no-preference", "value": "enabled" },
        }]));
        uuid
    }

    pub fn cancel_match(&self, uuid: &str) {
        self.send(json!(["automatch/cancel", { "uuid": uuid }]));
    }

    pub fn join_game(&self, game_id: u64) {
        self.send(json!(["game/connect", { "game_id": game_id, "chat": false }]));
    }

    pub fn leave_game(&self, game_id: u64) {
        self.send(json!(["game/disconnect", { "game_id": game_id }]));
    }

    pub fn play(&self, game_id: u64, point: Option<Point>) {
        self.send(json!(["game/move", { "game_id": game_id, "move": encode_move(point) }]));
    }

    pub fn resign(&self, game_id: u64) {
        self.send(json!(["game/resign", { "game_id": game_id }]));
    }

    /// Agrees to the dead stones OGS proposed at the end of the game.
    pub fn accept_score(&self, game_id: u64, all_removed: &str) {
        self.send(json!(["game/removed_stones/accept", {
            "game_id": game_id, "stones": all_removed, "strict_seki_mode": false,
        }]));
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_uuid() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_authenticate_reply() {
        assert_eq!(
            parse_frame(r#"[1,{"id":42,"username":"rafael"}]"#),
            Some(OgsEvent::Authenticated { user_id: 42, username: "rafael".into() })
        );
        assert!(matches!(
            parse_frame(r#"[1,null,{"code":"invalid_jwt","message":"Invalid token"}]"#),
            Some(OgsEvent::Error { .. })
        ));
    }

    #[test]
    fn reads_match_and_move_events() {
        assert_eq!(
            parse_frame(r#"["automatch/start",{"uuid":"u","game_id":77}]"#),
            Some(OgsEvent::MatchStarted { uuid: "u".into(), game_id: 77 })
        );
        assert_eq!(
            parse_frame(r#"["game/77/move",{"game_id":77,"move_number":3,"move":[15,3,1234]}]"#),
            Some(OgsEvent::Moved { game_id: 77, move_number: 3, point: Some(Point::new(15, 3)) })
        );
        assert_eq!(
            parse_frame(r#"["game/77/move",{"game_id":77,"move_number":4,"move":[-1,-1,500]}]"#),
            Some(OgsEvent::Moved { game_id: 77, move_number: 4, point: None })
        );
        assert_eq!(
            parse_frame(r#"["game/77/phase","stone removal"]"#),
            Some(OgsEvent::Phase { game_id: 77, phase: "stone removal".into() })
        );
        assert_eq!(parse_frame(r#"["net/pong",{"client":1,"server":2}]"#), None);
        assert_eq!(parse_frame("not json"), None);
    }

    fn sample_gamedata() -> String {
        r#"["game/77/gamedata",{"game_id":77,"width":9,"height":9,"komi":5.5,"phase":"play",
            "initial_player":"black","start_time":1762300000,"rules":"japanese",
            "players":{"black":{"id":42,"username":"rafael","rank":32.4},"white":{"id":7,"username":"opp","rank":28.1}},
            "initial_state":{"black":"","white":""},
            "moves":[[4,4,1000],[2,2,900],[-1,-1,800]]}]"#
            .to_string()
    }

    #[test]
    fn turns_gamedata_into_a_record() {
        let Some(OgsEvent::GameData { game }) = parse_frame(&sample_gamedata()) else { panic!("no gamedata") };
        assert_eq!(game.color_of(42), Some(Color::Black));
        let g = game.to_game();
        assert_eq!(g.info.size, 9);
        assert_eq!(g.info.black_rank.as_deref(), Some("3d"));
        assert_eq!(g.info.white_rank.as_deref(), Some("2k"));
        assert_eq!(g.info.server.as_deref(), Some("OGS"));
        assert!(g.info.time.is_some());
        assert_eq!(g.moves().len(), 3);
        assert_eq!(g.moves()[1], (Color::White, Move::Play { point: Point::new(2, 2) }));
        assert_eq!(g.to_move(), Color::White, "black passed last");
    }

    #[test]
    fn free_handicap_stones_are_all_black() {
        let data: GameData = serde_json::from_value(json!({
            "game_id": 1, "width": 19, "handicap": 2, "free_handicap_placement": true,
            "players": {"black": {"id": 1, "username": "b"}, "white": {"id": 2, "username": "w"}},
            "moves": [[3,3,0],[15,15,0],[15,3,0]]
        }))
        .unwrap();
        let g = data.to_game();
        let colors: Vec<Color> = g.moves().iter().map(|(c, _)| *c).collect();
        assert_eq!(colors, vec![Color::Black, Color::Black, Color::White]);
    }

    #[test]
    fn finished_games_get_a_result() {
        let data: GameData = serde_json::from_value(json!({
            "game_id": 1, "width": 19, "phase": "finished", "winner": 2, "outcome": "6.5 points",
            "players": {"black": {"id": 1, "username": "b"}, "white": {"id": 2, "username": "w"}}
        }))
        .unwrap();
        assert_eq!(data.result().as_deref(), Some("W+6.5"));
    }

    #[test]
    fn clocks_count_down_the_player_to_move() {
        let frame = r#"["game/77/clock",{"game_id":77,"current_player":42,"black_player_id":42,"white_player_id":7,
            "last_move":1000000,"now":1010000,"expiration":0,
            "black_time":{"thinking_time":5,"periods":3,"period_time":30},
            "white_time":{"thinking_time":300,"periods":5,"period_time":30}}]"#;
        let Some(OgsEvent::Clock { clock }) = parse_frame(frame) else { panic!("no clock") };
        assert_eq!(clock.to_move, Some(Color::Black));
        // 10 s elapsed: 5 s of main time, then 5 s into the first period.
        assert_eq!(clock.black.main, 0.0);
        assert_eq!(clock.black.periods, Some(3));
        assert_eq!(clock.black.period_left, Some(25.0));
        assert_eq!(clock.white.main, 300.0, "white's clock is not running");
    }

    #[test]
    fn running_out_of_periods() {
        let mut c = PlayerClock { main: 0.0, periods: Some(2), period_time: Some(10.0), period_left: None };
        c.spend(25.0);
        assert_eq!((c.periods, c.period_left), (Some(0), Some(0.0)));
    }

    #[test]
    fn encodes_moves_like_goban() {
        assert_eq!(encode_move(Some(Point::new(15, 3))), "pd");
        assert_eq!(encode_move(None), "..");
        assert_eq!(decode_points("pddpzz", 19), vec![Point::new(15, 3), Point::new(3, 15)]);
    }

    #[test]
    fn uuids_look_like_uuids() {
        let u = new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(u.chars().nth(14), Some('4'));
    }
}
