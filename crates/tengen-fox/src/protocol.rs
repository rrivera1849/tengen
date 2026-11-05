//! Fox messages to Tengen events, and the messages Tengen sends. No
//! networking here; see `PROTOCOL.md` for the message and field numbers.

use md5::{Digest, Md5};
use tengen_core::adapter::{Account, AdapterError, Server, Speed};
use tengen_core::live::{handicap_points, Clock, LiveEvent, LiveGame, Phase, PlayerClock};
use tengen_core::{Board, Color, Move, Point};

use crate::frame::{gateway_payload, split_gateway};
use crate::wire::{Msg, Pb};

pub const NAV_HOST: &str = "43.132.13.39";
pub const NAV_PORT: u16 = 10000;
const CLIENT_VERSION: u64 = 11_509_231;

// Server types.
pub const GATEWAY: u32 = 20001;
pub const ROOMS: u32 = 30006;
pub const GAMES: u32 = 31000;

// Navigation-server message types.
pub const NAV_LOGIN: u16 = 2000;
pub const NAV_LOGIN_REPLY: u16 = 2005;
pub const NAV_SERVERS: u16 = 3000;
pub const NAV_SERVERS_2: u16 = 3010;
pub const NAV_SERVERS_REPLY: u16 = 3005;

const WRONG_PASSWORD: i32 = 111_231;
const STALE_SYNC: i32 = 109_036;
const SIGNED_IN_ELSEWHERE: i32 = 51_001;

/// A message for the gateway: which server, which message, and its body.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub server: u32,
    pub id: u32,
    pub server_id: u64,
    pub body: Vec<u8>,
}

impl Out {
    fn new(server: u32, id: u32, body: Pb) -> Out {
        Out {
            server,
            id,
            server_id: 0,
            body: body.build(),
        }
    }

    fn to_game(id: u32, server_id: u64, body: Pb) -> Out {
        Out {
            server: GAMES,
            id,
            server_id,
            body: body.build(),
        }
    }

    /// The gateway payload, routed with the player's uid.
    pub fn encode(&self, uid: u64) -> Vec<u8> {
        let header = Pb::new()
            .int(1, u64::from(self.id))
            .int(3, uid)
            .int(5, 32) // PC client
            .int(6, 6)
            .int(7, u64::from(self.server))
            .int(8, self.server_id)
            .str(19, &CLIENT_VERSION.to_string())
            .int(20, 4)
            .build();
        gateway_payload(&header, &self.body)
    }
}

// ---------- the navigation server ----------

/// The first message: account name and the MD5 of the password.
pub fn login_request(name: &str, password: &str, mac: &str) -> Vec<u8> {
    let hash = hex(&Md5::digest(password.as_bytes()));
    Pb::new()
        .str(1, name)
        .str(2, "foxwq")
        .str(3, &hash)
        .int(4, 8)
        .int(5, CLIENT_VERSION)
        .bool(6, false)
        .str(7, mac)
        .bool(9, false)
        .build()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub uid: u64,
    pub token: Vec<u8>,
    pub account: Account,
}

pub fn login_reply(payload: &[u8]) -> Result<Login, AdapterError> {
    let m = Msg::decode(payload).map_err(|e| AdapterError::Protocol(e.0.into()))?;
    let code = m.i32(1).unwrap_or(0);
    if code == WRONG_PASSWORD {
        return Err(AdapterError::BadCredentials);
    }
    let account = m.sub(3);
    let uid = m
        .int(2)
        .filter(|&u| u != 0)
        .or_else(|| account.int(1))
        .unwrap_or(0);
    let token = m.bytes(8).unwrap_or_default().to_vec();
    if code != 0 || uid == 0 || token.is_empty() {
        return Err(AdapterError::Protocol(format!(
            "Fox refused the sign-in (code {code})"
        )));
    }
    let name = [account.text(3), account.text(2)]
        .into_iter()
        .find(|n| !n.is_empty())
        .unwrap_or_default();
    let rank = account.i32(4).and_then(rank_name);
    Ok(Login {
        uid,
        token,
        account: Account {
            server: Server::Fox,
            id: uid.to_string(),
            username: name,
            rank,
        },
    })
}

pub fn servers_request(uid: u64) -> [(u16, Vec<u8>); 2] {
    [
        (NAV_SERVERS, Pb::new().int(1, uid).bool(2, false).build()),
        (NAV_SERVERS_2, Pb::new().int(1, uid).build()),
    ]
}

/// The gateway's address from the server list.
pub fn gateway_address(payload: &[u8]) -> Option<(String, u16)> {
    let m = Msg::decode(payload).ok()?;
    let servers = m.subs(3);
    let record = servers.iter().find(|r| r.u(1) == 1).or(servers.first())?;
    let host = record.text(21);
    let port = u16::try_from(record.u(22)).ok().filter(|&p| p > 0)?;
    (!host.is_empty()).then_some((host, port))
}

/// Fox grades: 0–17 are 18k–1k, 18–26 are 1d–9d, 27–35 professional.
pub fn rank_name(grade: i32) -> Option<String> {
    match grade {
        0..=17 => Some(format!("{}k", 18 - grade)),
        18..=26 => Some(format!("{}d", grade - 17)),
        27..=35 => Some(format!("{}p", grade - 26)),
        _ => None,
    }
}

// ---------- the gateway ----------

/// One automatch setting ("field"): a board size and a time control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub id: u64,
    pub size: u8,
    pub main: u64,
    pub periods: u64,
    pub byoyomi: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoomKey {
    pub server_type: u64,
    pub server_id: u64,
    pub room_id: u64,
    pub created: u64,
}

impl RoomKey {
    fn pb(&self) -> Pb {
        Pb::new()
            .int(1, self.server_type)
            .int(2, self.server_id)
            .int(3, self.room_id)
            .int(4, self.created)
    }

    /// Tengen's id for the game.
    fn game_id(&self) -> u64 {
        self.room_id
    }
}

/// Where the counting handshake stands.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Counting {
    /// The opponent asked to count; accepting answers stage 1.
    Asked,
    /// Fox has judged the board; accepting confirms the result (stage 2).
    Judged,
}

#[derive(Debug, Default)]
struct FoxGame {
    room: Option<RoomKey>,
    size: u8,
    komi: f32,
    black: (String, Option<String>, u64),
    white: (String, Option<String>, u64),
    you: Option<Color>,
    handicap: Vec<Point>,
    moves: Vec<(Color, Move)>,
    announced: bool,
    sync: u64,
    period: Option<f64>,
    clocks: [PlayerClock; 2],
    phase: Phase,
    counting: Option<Counting>,
    asked_to_count: bool,
    ended: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Output {
    pub events: Vec<LiveEvent>,
    pub send: Vec<Out>,
}

/// The state of a signed-in Fox session.
#[derive(Debug)]
pub struct Translator {
    pub uid: u64,
    account: Account,
    token: Vec<u8>,
    pub fields: Vec<Field>,
    pub searching: bool,
    game: Option<FoxGame>,
}

impl Translator {
    pub fn new(login: Login) -> Self {
        Translator {
            uid: login.uid,
            account: login.account,
            token: login.token,
            fields: Vec::new(),
            searching: false,
            game: None,
        }
    }

    /// The first gateway message: present the session token.
    pub fn gateway_login(&self) -> Out {
        Out::new(GATEWAY, 1, Pb::new().bytes(1, &self.token))
    }

    /// Sent every five seconds to keep the session (and the game) alive.
    pub fn heartbeat(&self, ticks: u64) -> Vec<Out> {
        let mut out = vec![
            Out::new(GATEWAY, 5, Pb::new().int(1, ticks)),
            Out::new(ROOMS, 100_010, Pb::new().int(1, ticks)),
        ];
        if let Some(room) = self
            .game
            .as_ref()
            .and_then(|g| g.room)
            .filter(|_| self.in_game())
        {
            out.push(Out::to_game(
                200,
                room.server_id,
                Pb::new().int(1, ticks).msg(3, room.pb()),
            ));
        }
        out
    }

    fn in_game(&self) -> bool {
        self.game.as_ref().is_some_and(|g| !g.ended)
    }

    /// Handles one gateway packet.
    pub fn handle(&mut self, payload: &[u8]) -> Output {
        let mut out = Output::default();
        let Ok((head, body)) = split_gateway(payload) else {
            return out;
        };
        let (Ok(head), Ok(body)) = (Msg::decode(head), Msg::decode(body)) else {
            return out;
        };
        let (server, id) = (head.u(7) as u32, head.u(1) as u32);
        match (server, id) {
            (GATEWAY, 1) => match body.i32(1).unwrap_or(0) {
                0 => {
                    out.events.push(LiveEvent::Ready {
                        account: self.account.clone(),
                    });
                    out.send
                        .push(Out::new(ROOMS, 100_008, Pb::new().bool(1, true)));
                }
                code => out.events.push(LiveEvent::Disconnected {
                    reason: format!("Fox's gateway refused the session (code {code})"),
                    retry: true,
                }),
            },
            (GATEWAY, 100) => {
                let code = body.i32(1).unwrap_or(0);
                let mut reason = body.text(8).trim().to_string();
                if reason.is_empty() {
                    reason = if code == SIGNED_IN_ELSEWHERE {
                        "You signed in to Fox somewhere else".into()
                    } else {
                        format!("Fox ended the session (code {code})")
                    };
                }
                out.events.push(LiveEvent::Disconnected {
                    reason,
                    retry: code != SIGNED_IN_ELSEWHERE && body.u(11) == 0,
                });
            }
            (ROOMS, _) => self.rooms(id, &body, &mut out),
            (GAMES, _) => self.games(id, &head, &body, &mut out),
            _ => {}
        }
        out
    }

    fn rooms(&mut self, id: u32, m: &Msg, out: &mut Output) {
        match id {
            100_008 => {
                if m.i32(1).unwrap_or(0) != 0 {
                    out.events.push(LiveEvent::Notice(
                        "Fox's room server refused the sign-in".into(),
                    ));
                    return;
                }
                for request in [100_115, 100_150] {
                    out.send.push(Out::new(ROOMS, request, Pb::new()));
                }
            }
            100_150 => {
                self.fields = m
                    .subs(1)
                    .iter()
                    .filter(|f| f.u(12) == 1 && f.u(7) == 0)
                    .map(|f| Field {
                        id: f.u(1),
                        size: f.u(2) as u8,
                        main: f.u(3),
                        periods: f.u(4),
                        byoyomi: f.u(5),
                    })
                    .filter(|f| {
                        f.id > 0
                            && (if f.periods == 0 {
                                f.main > 0
                            } else {
                                f.byoyomi > 0
                            })
                    })
                    .collect();
            }
            // An unfinished game from before: pick it back up.
            100_115 => {
                if m.i32(1).unwrap_or(-1) == 0 && m.u(2) == u64::from(GAMES) {
                    let room = RoomKey {
                        server_type: m.u(2),
                        server_id: m.u(3),
                        room_id: m.u(4),
                        created: m.u(7),
                    };
                    self.game = Some(FoxGame {
                        room: Some(room),
                        ..Default::default()
                    });
                    out.send.push(Out::to_game(501, room.server_id, Pb::new()));
                }
            }
            100_130 => {
                let result = m.i32(1).unwrap_or(0);
                if result != 0 && self.searching {
                    self.searching = false;
                    out.events.push(LiveEvent::SearchStopped);
                    out.events.push(LiveEvent::Notice(format!(
                        "Fox didn't start automatch (code {result})"
                    )));
                }
            }
            100_132 | 100_139 | 100_141 => {
                if self.searching {
                    self.searching = false;
                    out.events.push(LiveEvent::SearchStopped);
                    if id == 100_132 {
                        out.events
                            .push(LiveEvent::Notice("No opponent found on Fox in time".into()));
                    }
                }
            }
            // Opponent found: a room exists; say we're ready.
            100_140 => {
                if m.i32(1).unwrap_or(0) != 0 {
                    out.events
                        .push(LiveEvent::Notice("Fox couldn't open the game room".into()));
                    return;
                }
                let room = RoomKey {
                    server_type: m.u(2),
                    server_id: m.u(3),
                    room_id: m.u(4),
                    created: m.u(7),
                };
                if self.searching {
                    self.searching = false;
                    out.events.push(LiveEvent::SearchStopped);
                }
                self.game = Some(FoxGame {
                    room: Some(room),
                    ..Default::default()
                });
                out.send.push(Out::to_game(105, room.server_id, Pb::new()));
            }
            _ => {}
        }
    }

    fn games(&mut self, id: u32, head: &Msg, m: &Msg, out: &mut Output) {
        let my_uid = self.uid;
        match id {
            // The game starts.
            107 => {
                let room = RoomKey {
                    server_type: m.u(11).max(u64::from(GAMES)),
                    server_id: m.u(12).max(head.u(8)),
                    room_id: m.u(6),
                    created: m.u(7),
                };
                let mut g = FoxGame {
                    room: Some(room),
                    ..Default::default()
                };
                read_rule(&mut g, &m.sub(1));
                read_players(&mut g, &m.subs(9), my_uid);
                read_moves(&mut g, &m.subs(18));
                self.game = Some(g);
                let g = self.game.as_mut().expect("game");
                announce(g, out);
                force_phase(g, Phase::Play, out);
                out.events.extend(g.clock());
            }
            // A stone.
            203 => {
                let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
                    return;
                };
                let color = if m.u(3) == 2 {
                    Color::White
                } else {
                    Color::Black
                };
                let point = Point::new(m.u(1) as u8, m.u(2) as u8);
                g.sync = m.sub(5).int(1).unwrap_or(g.sync);
                read_countdown(g, color, &m.sub(4));
                g.asked_to_count = false;
                push_move(g, color, Move::Play { point }, out);
                out.events.extend(g.clock());
            }
            // A pass.
            206 => {
                let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
                    return;
                };
                let color = g.to_move();
                g.sync = m.sub(2).int(1).unwrap_or(g.sync);
                read_countdown(g, color, &m.sub(1));
                push_move(g, color, Move::Pass, out);
                out.events.extend(g.clock());
                // Two passes in a row: ask Fox to count, as its own client offers.
                let n = g.moves.len();
                if n >= 2
                    && g.moves[n - 1].1 == Move::Pass
                    && g.moves[n - 2].1 == Move::Pass
                    && !g.asked_to_count
                    && g.counting.is_none()
                {
                    if let Some(room) = g.room {
                        g.asked_to_count = true;
                        out.send.push(Out::to_game(
                            301,
                            room.server_id,
                            Pb::new().int(1, 0).msg(2, room.pb()),
                        ));
                        set_phase(g, Phase::Scoring, out);
                        out.events.push(LiveEvent::Notice(
                            "Both passed. Fox will count once your opponent agrees".into(),
                        ));
                    }
                }
            }
            // Clock updates.
            108 | 109 => {
                let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
                    return;
                };
                let c = if id == 109 { m.sub(2) } else { m.clone() };
                if c.has(5) {
                    let color = if c.u(5) == g.black.2 {
                        Color::Black
                    } else {
                        Color::White
                    };
                    read_countdown(g, color, &c);
                    out.events.extend(g.clock());
                }
            }
            // Counting: asked, or judged.
            303 => {
                let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
                    return;
                };
                let result = m.i32(1).unwrap_or(0);
                if result != 0 {
                    g.counting = None;
                    set_phase(g, Phase::Play, out);
                    out.events.push(LiveEvent::Notice(format!(
                        "Fox couldn't count the game (code {result}). Play on"
                    )));
                    return;
                }
                match m.u(2) {
                    1 => {
                        g.counting = Some(Counting::Asked);
                        set_phase(g, Phase::Scoring, out);
                        out.events.push(LiveEvent::Notice(
                            "Your opponent wants to count the game. Accept score to agree".into(),
                        ));
                    }
                    2 => {
                        g.counting = Some(Counting::Judged);
                        set_phase(g, Phase::Scoring, out);
                        let dead = dead_stones(g, m.bytes(5).unwrap_or_default());
                        if let Some(room) = g.room {
                            out.events.push(LiveEvent::Dead {
                                game_id: room.game_id(),
                                points: dead,
                            });
                        }
                        let winner = match m.u(8) {
                            1 => "Black wins",
                            2 => "White wins",
                            3 => "It's a draw",
                            _ => "Fox has a result",
                        };
                        out.events.push(LiveEvent::Notice(format!(
                            "Fox counted the game: {winner}. Accept score to confirm"
                        )));
                    }
                    _ => {}
                }
            }
            // The other side answered counting.
            306 => {
                let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
                    return;
                };
                if m.u(1) == 0 {
                    g.counting = None;
                    g.asked_to_count = false;
                    set_phase(g, Phase::Play, out);
                    if let Some(room) = g.room {
                        out.events.push(LiveEvent::Dead {
                            game_id: room.game_id(),
                            points: Vec::new(),
                        });
                    }
                    out.events
                        .push(LiveEvent::Notice("Counting was declined. Play on".into()));
                }
            }
            // Answers to our own requests: only errors matter.
            201 | 204 | 301 | 304 | 311 | 100_002 => {
                let code = m.i32(1).unwrap_or(0); // result, or the error code of 100002
                if code == STALE_SYNC {
                    if let Some(room) = self.game.as_ref().and_then(|g| g.room) {
                        out.send.push(Out::to_game(
                            501,
                            room.server_id,
                            Pb::new().msg(1, room.pb()),
                        ));
                    }
                } else if code != 0 {
                    let what = match id {
                        201 => "That move",
                        204 => "The pass",
                        311 => "Resigning",
                        _ => "That request",
                    };
                    out.events.push(LiveEvent::Notice(format!(
                        "{what} was refused by Fox (code {code})"
                    )));
                }
            }
            // Game over.
            402 | 403 | 405 => {
                let Some(g) = self.game.as_mut() else { return };
                if g.ended {
                    return;
                }
                let points = if m.has(3) { m.i32(3) } else { None };
                if let Some(result) = result_text(m.u(2), points, m.u(4)) {
                    end(g, result, out);
                }
            }
            // The whole game, after a reconnect or a sync error.
            501 => {
                if m.i32(1).unwrap_or(0) != 0 {
                    return;
                }
                let room = RoomKey {
                    server_type: m.u(12).max(u64::from(GAMES)),
                    server_id: m.u(13).max(head.u(8)),
                    room_id: m.u(7),
                    created: m.u(10),
                };
                let mut g = FoxGame {
                    room: Some(room),
                    ..Default::default()
                };
                read_rule(&mut g, &m.sub(2));
                read_players(&mut g, &m.subs(3), my_uid);
                read_moves(&mut g, &m.subs(4));
                g.sync = m.sub(5).int(1).unwrap_or(0);
                if let Some(old) = self
                    .game
                    .as_ref()
                    .filter(|o| o.room.map(|r| r.room_id) == Some(room.room_id))
                {
                    g.counting = old.counting;
                }
                let over = m.u(6) == 5;
                self.game = Some(g);
                let g = self.game.as_mut().expect("game");
                announce(g, out);
                if over {
                    let r = m.sub(15);
                    let points = if r.has(2) { r.i32(2) } else { None };
                    let result = result_text(r.u(1), points, r.u(3)).unwrap_or_else(|| "?".into());
                    end(g, result, out);
                } else {
                    force_phase(
                        g,
                        if g.counting.is_some() {
                            Phase::Scoring
                        } else {
                            Phase::Play
                        },
                        out,
                    );
                    out.events.extend(g.clock());
                }
            }
            _ => {}
        }
    }

    // ---------- commands ----------

    /// Starts automatch on the field closest to the size and pace asked for.
    pub fn automatch(&mut self, size: u8, speed: Speed) -> Result<Out, String> {
        if self.fields.is_empty() {
            return Err("Fox hasn't sent its game settings yet. Try again in a moment.".into());
        }
        let mut fields: Vec<Field> = self
            .fields
            .iter()
            .copied()
            .filter(|f| f.size == size)
            .collect();
        if fields.is_empty() {
            return Err(format!(
                "Fox has no automatch for {size} × {size} right now"
            ));
        }
        fields.sort_by_key(|f| (f.main + f.periods * f.byoyomi, f.byoyomi, f.id));
        let pick = match speed {
            Speed::Blitz => fields[0],
            Speed::Rapid => fields[fields.len() / 2],
            Speed::Live | Speed::Correspondence => fields[fields.len() - 1],
        };
        self.searching = true;
        Ok(Out::new(
            ROOMS,
            100_130,
            Pb::new().int(1, pick.id).bool(2, false),
        ))
    }

    pub fn cancel_automatch(&mut self) -> Out {
        self.searching = false;
        Out::new(ROOMS, 100_139, Pb::new())
    }

    fn current(&self, game_id: u64) -> Option<(&FoxGame, RoomKey)> {
        let g = self.game.as_ref().filter(|g| !g.ended)?;
        let room = g.room.filter(|r| r.game_id() == game_id)?;
        Some((g, room))
    }

    pub fn play(&self, game_id: u64, mv: Move) -> Option<Out> {
        let (g, room) = self.current(game_id)?;
        let sync = Pb::new().int(1, g.sync);
        Some(match mv {
            Move::Play { point } => Out::to_game(
                201,
                room.server_id,
                Pb::new()
                    .int(1, u64::from(point.x))
                    .int(2, u64::from(point.y))
                    .int(3, color_code(g.you?))
                    .msg(4, sync),
            ),
            Move::Pass => Out::to_game(204, room.server_id, Pb::new().msg(1, sync)),
        })
    }

    pub fn resign(&self, game_id: u64) -> Option<Out> {
        let (g, room) = self.current(game_id)?;
        Some(Out::to_game(
            311,
            room.server_id,
            Pb::new().int(1, color_code(g.you?)).msg(2, room.pb()),
        ))
    }

    /// Accept score: agree to count, confirm Fox's count, or ask to count.
    pub fn accept_score(&mut self, game_id: u64) -> Option<Out> {
        let g = self.game.as_mut().filter(|g| !g.ended)?;
        let room = g.room.filter(|r| r.game_id() == game_id)?;
        Some(match g.counting {
            Some(stage) => {
                let stage = if stage == Counting::Asked { 1 } else { 2 };
                Out::to_game(
                    304,
                    room.server_id,
                    Pb::new().bool(1, true).int(2, stage).msg(3, room.pb()),
                )
            }
            None => {
                g.asked_to_count = true;
                Out::to_game(301, room.server_id, Pb::new().int(1, 0).msg(2, room.pb()))
            }
        })
    }

    pub fn leave(&mut self, game_id: u64) {
        if self
            .game
            .as_ref()
            .and_then(|g| g.room)
            .is_some_and(|r| r.game_id() == game_id)
        {
            self.game = None;
        }
    }
}

fn color_code(c: Color) -> u64 {
    if c == Color::White {
        2
    } else {
        1
    }
}

impl FoxGame {
    fn to_move(&self) -> Color {
        match self.moves.last() {
            Some((c, _)) => c.opposite(),
            None if !self.handicap.is_empty() => Color::White,
            None => Color::Black,
        }
    }

    fn clock(&self) -> Option<LiveEvent> {
        let room = self.room?;
        Some(LiveEvent::Clock(Clock {
            game_id: room.game_id(),
            to_move: (self.phase == Phase::Play).then(|| self.to_move()),
            black: self.clocks[0],
            white: self.clocks[1],
            paused: self.phase != Phase::Play,
        }))
    }

    fn board(&self) -> Board {
        let mut b = Board::new(self.size.max(2));
        for p in &self.handicap {
            b.set(*p, Some(Color::Black));
        }
        for (c, mv) in &self.moves {
            if let Move::Play { point } = mv {
                if b.play(*point, *c).is_err() {
                    b.set(*point, Some(*c));
                }
            }
        }
        b
    }
}

fn read_rule(g: &mut FoxGame, rule: &Msg) {
    g.size = rule
        .int(1)
        .map(|s| s as u8)
        .filter(|&s| (5..=25).contains(&s))
        .unwrap_or(19);
    // Fox keeps komi as points × 50 (375 is 7.5).
    g.komi = rule
        .i32(4)
        .map(|k| {
            if k.abs() > 100 {
                k as f32 / 50.0
            } else {
                k as f32
            }
        })
        .unwrap_or(7.5);
    g.period = rule.int(7).filter(|&s| s > 0).map(|s| s as f64);
    let handicap = rule.u(3) as u8;
    if handicap >= 2 {
        g.handicap = handicap_points(g.size, handicap);
    }
}

fn read_players(g: &mut FoxGame, users: &[Msg], my_uid: u64) {
    for u in users {
        let color = match u.u(6) {
            1 => Color::Black,
            2 => Color::White,
            _ => continue,
        };
        let name = [u.text(3), u.text(19), u.text(2)]
            .into_iter()
            .find(|n| !n.is_empty())
            .unwrap_or_default();
        let entry = (name, u.i32(4).and_then(rank_name), u.u(1));
        if u.u(1) == my_uid {
            g.you = Some(color);
        }
        let clock = player_clock(u.u(7), u.u(8), u.u(9), g.period);
        match color {
            Color::Black => {
                g.black = entry;
                g.clocks[0] = clock;
            }
            Color::White => {
                g.white = entry;
                g.clocks[1] = clock;
            }
        }
    }
}

/// Moves listed with a game (start or full fetch): handicap, stones, passes.
fn read_moves(g: &mut FoxGame, moves: &[Msg]) {
    let mut handicap = Vec::new();
    for m in moves {
        let color = if m.u(4) == 2 {
            Color::White
        } else {
            Color::Black
        };
        let point = Point::new(m.u(2) as u8, m.u(3) as u8);
        match m.u(1) {
            1 => g.moves.push((color, Move::Play { point })),
            2 => g.moves.push((color, Move::Pass)),
            3 => handicap.push(point),
            _ => {}
        }
    }
    if !handicap.is_empty() {
        g.handicap = handicap;
    }
}

fn player_clock(main: u64, periods: u64, left: u64, period: Option<f64>) -> PlayerClock {
    PlayerClock {
        main: main as f64,
        periods: period.map(|_| periods as u32),
        period_time: period,
        period_left: (main == 0 && period.is_some()).then_some(left as f64),
        canadian: false,
    }
}

fn read_countdown(g: &mut FoxGame, color: Color, c: &Msg) {
    if !c.has(2) {
        return;
    }
    let clock = player_clock(c.u(2), c.u(3), c.u(4), g.period);
    g.clocks[if color == Color::Black { 0 } else { 1 }] = clock;
}

fn push_move(g: &mut FoxGame, color: Color, mv: Move, out: &mut Output) {
    g.moves.push((color, mv));
    match g.room {
        Some(room) if g.announced => out.events.push(LiveEvent::Moved {
            game_id: room.game_id(),
            color,
            mv,
        }),
        _ => announce(g, out),
    }
}

fn announce(g: &mut FoxGame, out: &mut Output) {
    let Some(room) = g.room else { return };
    g.announced = true;
    out.events.push(LiveEvent::Game(LiveGame {
        id: room.game_id(),
        size: g.size,
        komi: g.komi,
        black: g.black.0.clone(),
        white: g.white.0.clone(),
        black_rank: g.black.1.clone(),
        white_rank: g.white.1.clone(),
        you: g.you,
        handicap: g.handicap.clone(),
        moves: g.moves.clone(),
    }));
}

fn set_phase(g: &mut FoxGame, phase: Phase, out: &mut Output) {
    if g.phase == phase {
        return;
    }
    g.phase = phase;
    if let Some(room) = g.room {
        out.events.push(LiveEvent::Phase {
            game_id: room.game_id(),
            phase,
        });
    }
}

/// Sets the phase and always reports it (a newly opened game needs it said).
fn force_phase(g: &mut FoxGame, phase: Phase, out: &mut Output) {
    g.phase = phase;
    if let Some(room) = g.room {
        out.events.push(LiveEvent::Phase {
            game_id: room.game_id(),
            phase,
        });
    }
}

fn end(g: &mut FoxGame, result: String, out: &mut Output) {
    if g.ended {
        return;
    }
    g.ended = true;
    g.counting = None;
    set_phase(g, Phase::Finished, out);
    if let Some(room) = g.room {
        out.events.push(LiveEvent::Ended {
            game_id: room.game_id(),
            result,
        });
    }
}

/// Stones Fox's judgement gives to the other side: those are dead.
fn dead_stones(g: &FoxGame, ownership: &[u8]) -> Vec<Point> {
    let n = g.size as usize;
    if ownership.len() < n * n {
        return Vec::new();
    }
    let board = g.board();
    let mut dead: Vec<Point> = board
        .stones()
        .filter(|(p, c)| {
            let owner = ownership[p.y as usize * n + p.x as usize];
            (owner == b'1' && *c == Color::White) || (owner == b'2' && *c == Color::Black)
        })
        .map(|(p, _)| p)
        .collect();
    dead.sort_by_key(|p| (p.y, p.x));
    dead
}

/// Fox's end of game in SGF form: winner 1 black, 2 white, 3 draw; points
/// are × 50, with −2 meaning a loss on time; reason 3 is a resignation and
/// 8 a game too short to count.
pub fn result_text(winner: u64, points: Option<i32>, reason: u64) -> Option<String> {
    if reason == 8 {
        return Some("Void".into());
    }
    if winner == 3 {
        return Some("0".into());
    }
    let w = match winner {
        1 => "B",
        2 => "W",
        _ => return None,
    };
    let how = if reason == 3 {
        "R".to_string()
    } else if points == Some(-2) || reason == 2 {
        "T".to_string()
    } else if let Some(p) = points.filter(|&p| p >= 0) {
        let v = p as f32 / 50.0;
        if v.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v}")
        }
    } else {
        String::new()
    };
    Some(format!("{w}+{how}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> Login {
        Login {
            uid: 777,
            token: b"tok".to_vec(),
            account: Account {
                server: Server::Fox,
                id: "777".into(),
                username: "rafael".into(),
                rank: Some("15k".into()),
            },
        }
    }

    /// A gateway packet as Fox would send it.
    fn packet(server: u32, id: u32, server_id: u64, body: Pb) -> Vec<u8> {
        Out {
            server,
            id,
            server_id,
            body: body.build(),
        }
        .encode(1)
    }

    fn signed_in() -> Translator {
        let mut t = Translator::new(login());
        let out = t.handle(&packet(GATEWAY, 1, 0, Pb::new().int(1, 0)));
        assert!(
            matches!(&out.events[0], LiveEvent::Ready { account } if account.username == "rafael")
        );
        assert_eq!((out.send[0].server, out.send[0].id), (ROOMS, 100_008));
        let out = t.handle(&packet(ROOMS, 100_008, 0, Pb::new().int(1, 0)));
        assert_eq!(
            out.send.iter().map(|o| o.id).collect::<Vec<_>>(),
            vec![100_115, 100_150]
        );
        let field = |id, size, main, periods, byo| {
            Pb::new()
                .int(1, id)
                .int(2, size)
                .int(3, main)
                .int(4, periods)
                .int(5, byo)
                .int(7, 0)
                .int(12, 1)
        };
        t.handle(&packet(
            ROOMS,
            100_150,
            0,
            Pb::new()
                .msg(1, field(11, 19, 60, 3, 10))
                .msg(1, field(12, 19, 1200, 3, 60))
                .msg(1, field(13, 19, 300, 3, 30))
                .msg(1, field(14, 9, 60, 3, 10))
                .msg(1, Pb::new().int(1, 99).int(2, 19).int(3, 60).int(12, 7)), // a variant, skipped
        ));
        t
    }

    fn player(uid: u64, name: &str, grade: i32, color: u64) -> Pb {
        Pb::new()
            .int(1, uid)
            .str(3, name)
            .i32(4, grade)
            .int(6, color)
            .int(7, 300)
            .int(8, 3)
            .int(9, 30)
    }

    fn started() -> Translator {
        let mut t = signed_in();
        t.searching = true;
        let out = t.handle(&packet(
            ROOMS,
            100_140,
            0,
            Pb::new()
                .int(1, 0)
                .int(2, 31000)
                .int(3, 8)
                .int(4, 4242)
                .int(7, 1700),
        ));
        assert_eq!(out.events, vec![LiveEvent::SearchStopped]);
        assert_eq!((out.send[0].id, out.send[0].server_id), (105, 8));
        let rule = Pb::new()
            .int(1, 19)
            .int(4, 375)
            .int(6, 300)
            .int(7, 30)
            .int(8, 3);
        let out = t.handle(&packet(
            GAMES,
            107,
            8,
            Pb::new()
                .msg(1, rule)
                .int(6, 4242)
                .int(7, 1700)
                .int(11, 31000)
                .int(12, 8)
                .msg(9, player(777, "rafael", 3, 1))
                .msg(9, player(555, "kumo", 5, 2)),
        ));
        let LiveEvent::Game(g) = &out.events[0] else {
            panic!("{:?}", out.events)
        };
        assert_eq!(
            (g.id, g.size, g.komi, g.you),
            (4242, 19, 7.5, Some(Color::Black))
        );
        assert_eq!(
            (
                g.black_rank.as_deref(),
                g.white.as_str(),
                g.white_rank.as_deref()
            ),
            (Some("15k"), "kumo", Some("13k"))
        );
        assert!(out.events.contains(&LiveEvent::Phase {
            game_id: 4242,
            phase: Phase::Play
        }));
        t
    }

    #[test]
    fn signing_in_reads_the_name_rank_and_token() {
        let reply = Pb::new()
            .int(1, 0)
            .int(2, 777)
            .msg(3, Pb::new().int(1, 777).str(3, "rafael").int(4, 3))
            .bytes(8, b"tok")
            .build();
        let l = login_reply(&reply).unwrap();
        assert_eq!(
            (
                l.uid,
                l.account.username.as_str(),
                l.account.rank.as_deref(),
                l.token.as_slice()
            ),
            (777, "rafael", Some("15k"), &b"tok"[..])
        );
        let wrong = Pb::new().i32(1, WRONG_PASSWORD).build();
        assert!(matches!(
            login_reply(&wrong),
            Err(AdapterError::BadCredentials)
        ));
    }

    #[test]
    fn the_password_is_sent_as_md5_hex() {
        let m = Msg::decode(&login_request("rafael", "password", "AABBCCDDEEFF")).unwrap();
        assert_eq!(m.text(3), "5f4dcc3b5aa765d61d8327deb882cf99");
        assert_eq!(
            (m.text(1).as_str(), m.text(2).as_str(), m.u(5)),
            ("rafael", "foxwq", CLIENT_VERSION)
        );
    }

    #[test]
    fn the_gateway_comes_from_the_server_list() {
        let list = Pb::new()
            .msg(3, Pb::new().int(1, 2).str(21, "other").int(22, 1))
            .msg(3, Pb::new().int(1, 1).str(21, "gw.fox").int(22, 8000))
            .build();
        assert_eq!(gateway_address(&list), Some(("gw.fox".into(), 8000)));
    }

    #[test]
    fn automatch_picks_a_field_by_size_and_pace() {
        let mut t = signed_in();
        assert_eq!(t.fields.len(), 4);
        let id = |o: Out| Msg::decode(&o.body).unwrap().u(1);
        assert_eq!(id(t.automatch(19, Speed::Blitz).unwrap()), 11);
        assert_eq!(id(t.automatch(19, Speed::Rapid).unwrap()), 13);
        assert_eq!(id(t.automatch(19, Speed::Live).unwrap()), 12);
        assert_eq!(id(t.automatch(9, Speed::Live).unwrap()), 14);
        assert!(t.automatch(13, Speed::Live).is_err());
        assert!(t.searching);
    }

    #[test]
    fn moves_passes_and_clocks_follow_the_game() {
        let mut t = started();
        let out = t.handle(&packet(
            GAMES,
            203,
            8,
            Pb::new()
                .int(1, 15)
                .int(2, 3)
                .int(3, 1)
                .msg(4, Pb::new().int(2, 290).int(3, 3).int(4, 30))
                .msg(5, Pb::new().int(1, 1)),
        ));
        assert_eq!(
            out.events[0],
            LiveEvent::Moved {
                game_id: 4242,
                color: Color::Black,
                mv: Move::Play {
                    point: Point::new(15, 3)
                }
            }
        );
        let LiveEvent::Clock(c) = &out.events[1] else {
            panic!()
        };
        assert_eq!((c.to_move, c.black.main), (Some(Color::White), 290.0));
        // Our next move carries the sync id Fox last gave.
        let out = t.handle(&packet(
            GAMES,
            206,
            8,
            Pb::new()
                .msg(1, Pb::new().int(2, 0).int(3, 2).int(4, 25))
                .msg(2, Pb::new().int(1, 2)),
        ));
        assert_eq!(
            out.events[0],
            LiveEvent::Moved {
                game_id: 4242,
                color: Color::White,
                mv: Move::Pass
            }
        );
        let LiveEvent::Clock(c) = &out.events[1] else {
            panic!()
        };
        assert_eq!(
            c.white,
            PlayerClock {
                main: 0.0,
                periods: Some(2),
                period_time: Some(30.0),
                period_left: Some(25.0),
                canadian: false
            }
        );
        let send = t
            .play(
                4242,
                Move::Play {
                    point: Point::new(3, 3),
                },
            )
            .unwrap();
        let m = Msg::decode(&send.body).unwrap();
        assert_eq!(
            (
                send.id,
                send.server_id,
                m.u(1),
                m.u(2),
                m.u(3),
                m.sub(4).u(1)
            ),
            (201, 8, 3, 3, 1, 2)
        );
    }

    #[test]
    fn two_passes_ask_fox_to_count() {
        let mut t = started();
        t.handle(&packet(
            GAMES,
            206,
            8,
            Pb::new().msg(2, Pb::new().int(1, 1)),
        ));
        let out = t.handle(&packet(
            GAMES,
            206,
            8,
            Pb::new().msg(2, Pb::new().int(1, 2)),
        ));
        assert!(out.send.iter().any(|o| o.id == 301));
        assert!(out.events.contains(&LiveEvent::Phase {
            game_id: 4242,
            phase: Phase::Scoring
        }));
    }

    #[test]
    fn counting_shows_dead_stones_and_accepting_confirms() {
        let mut t = started();
        t.handle(&packet(
            GAMES,
            203,
            8,
            Pb::new().int(1, 0).int(2, 0).int(3, 1),
        ));
        t.handle(&packet(
            GAMES,
            203,
            8,
            Pb::new().int(1, 18).int(2, 18).int(3, 2),
        ));
        // The opponent asks to count.
        let out = t.handle(&packet(GAMES, 303, 8, Pb::new().int(1, 0).int(2, 1)));
        assert!(out.events.contains(&LiveEvent::Phase {
            game_id: 4242,
            phase: Phase::Scoring
        }));
        let answer = t.accept_score(4242).unwrap();
        assert_eq!(
            (answer.id, Msg::decode(&answer.body).unwrap().u(2)),
            (304, 1)
        );
        // Fox judges: the whole board is Black's, so White's stone is dead.
        let ownership = "1".repeat(361);
        let out = t.handle(&packet(
            GAMES,
            303,
            8,
            Pb::new().int(1, 0).int(2, 2).str(5, &ownership).int(8, 1),
        ));
        assert_eq!(
            out.events[0],
            LiveEvent::Dead {
                game_id: 4242,
                points: vec![Point::new(18, 18)]
            }
        );
        let answer = t.accept_score(4242).unwrap();
        assert_eq!(Msg::decode(&answer.body).unwrap().u(2), 2);
        let out = t.handle(&packet(
            GAMES,
            402,
            8,
            Pb::new().int(2, 1).i32(3, 1625).int(4, 1),
        ));
        assert_eq!(
            out.events,
            vec![
                LiveEvent::Phase {
                    game_id: 4242,
                    phase: Phase::Finished
                },
                LiveEvent::Ended {
                    game_id: 4242,
                    result: "B+32.5".into()
                },
            ]
        );
        assert!(t
            .handle(&packet(GAMES, 403, 8, Pb::new().int(2, 1).i32(3, 1625)))
            .events
            .is_empty());
    }

    #[test]
    fn declined_counting_goes_back_to_play() {
        let mut t = started();
        t.handle(&packet(GAMES, 303, 8, Pb::new().int(2, 1)));
        let out = t.handle(&packet(GAMES, 306, 8, Pb::new().int(1, 0).int(2, 1)));
        assert!(out.events.contains(&LiveEvent::Phase {
            game_id: 4242,
            phase: Phase::Play
        }));
    }

    #[test]
    fn results_read_like_sgf() {
        assert_eq!(result_text(2, None, 3).as_deref(), Some("W+R"));
        assert_eq!(result_text(1, Some(-2), 2).as_deref(), Some("B+T"));
        assert_eq!(result_text(2, Some(175), 1).as_deref(), Some("W+3.5"));
        assert_eq!(result_text(3, None, 1).as_deref(), Some("0"));
        assert_eq!(result_text(1, None, 8).as_deref(), Some("Void"));
    }

    #[test]
    fn a_stale_move_fetches_the_whole_game_again() {
        let mut t = started();
        let out = t.handle(&packet(GAMES, 201, 8, Pb::new().i32(1, STALE_SYNC)));
        assert_eq!(out.send[0].id, 501);
        // The full game replaces the record, moves and all.
        let out = t.handle(&packet(
            GAMES,
            501,
            8,
            Pb::new()
                .int(1, 0)
                .msg(2, Pb::new().int(1, 19).int(4, 375).int(7, 30))
                .msg(3, player(777, "rafael", 3, 1))
                .msg(3, player(555, "kumo", 5, 2))
                .msg(4, Pb::new().int(1, 1).int(2, 3).int(3, 3).int(4, 1))
                .msg(4, Pb::new().int(1, 2).int(4, 2))
                .msg(5, Pb::new().int(1, 9))
                .int(6, 3)
                .int(7, 4242)
                .int(10, 1700)
                .int(12, 31000)
                .int(13, 8),
        ));
        let LiveEvent::Game(g) = &out.events[0] else {
            panic!("{:?}", out.events)
        };
        assert_eq!(
            g.moves,
            vec![
                (
                    Color::Black,
                    Move::Play {
                        point: Point::new(3, 3)
                    }
                ),
                (Color::White, Move::Pass)
            ]
        );
        let pass = t.play(4242, Move::Pass).unwrap();
        assert_eq!(Msg::decode(&pass.body).unwrap().sub(1).u(1), 9);
    }

    #[test]
    fn an_unfinished_game_is_resumed_on_sign_in() {
        let mut t = signed_in();
        let out = t.handle(&packet(
            ROOMS,
            100_115,
            0,
            Pb::new()
                .int(1, 0)
                .int(2, 31000)
                .int(3, 8)
                .int(4, 4242)
                .int(7, 1700),
        ));
        assert_eq!((out.send[0].id, out.send[0].server_id), (501, 8));
    }

    #[test]
    fn signing_in_elsewhere_ends_the_session_for_good() {
        let mut t = signed_in();
        let out = t.handle(&packet(
            GATEWAY,
            100,
            0,
            Pb::new().i32(1, SIGNED_IN_ELSEWHERE),
        ));
        assert!(matches!(
            &out.events[0],
            LiveEvent::Disconnected { retry: false, .. }
        ));
    }

    #[test]
    fn heartbeats_include_the_game_while_playing() {
        let t = started();
        let beats = t.heartbeat(1000);
        assert_eq!(
            beats.iter().map(|o| (o.server, o.id)).collect::<Vec<_>>(),
            vec![(GATEWAY, 5), (ROOMS, 100_010), (GAMES, 200)]
        );
        assert_eq!(beats[2].server_id, 8);
    }

    #[test]
    fn ranks_follow_fox_grades() {
        assert_eq!(rank_name(0).as_deref(), Some("18k"));
        assert_eq!(rank_name(17).as_deref(), Some("1k"));
        assert_eq!(rank_name(18).as_deref(), Some("1d"));
        assert_eq!(rank_name(27).as_deref(), Some("1p"));
        assert_eq!(rank_name(-1), None);
    }

    #[test]
    fn resigning_names_your_color_and_the_room() {
        let t = started();
        let r = t.resign(4242).unwrap();
        let m = Msg::decode(&r.body).unwrap();
        assert_eq!((r.id, m.u(1), m.sub(2).u(3)), (311, 1, 4242));
        assert!(t.resign(1).is_none());
    }
}
