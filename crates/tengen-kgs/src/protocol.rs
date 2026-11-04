//! Turns KGS JSON messages into Tengen events, and builds the messages Tengen
//! sends. No networking here, so every rule can be tested with plain JSON.
//!
//! Protocol reference: <https://www.gokgs.com/json/protocol.html>.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use tengen_core::adapter::{Account, Server, Speed};
use tengen_core::live::{Clock, LiveEvent, LiveGame, Phase, PlayerClock};
use tengen_core::{Color, Move, Point};

/// What one batch of downstream messages produced.
#[derive(Debug, Default, PartialEq)]
pub struct Output {
    pub events: Vec<LiveEvent>,
    /// Messages KGS expects back right away (answers to connection checks).
    pub replies: Vec<Value>,
}

/// The state of the session, built up from everything KGS has said.
#[derive(Debug, Default)]
pub struct Translator {
    me: Option<String>,
    my_rank: Option<String>,
    prefs: Option<Map<String, Value>>,
    pub searching: bool,
    games: HashMap<u64, GameState>,
    /// Set when we asked to log out, so the goodbye isn't reported as a drop.
    pub logging_out: bool,
}

/// Handicap stones and the moves after them.
type Record = (Vec<Point>, Vec<(Color, Move)>);

#[derive(Debug, Default)]
struct GameState {
    tree: Tree,
    black: Option<(String, Option<String>)>,
    white: Option<(String, Option<String>)>,
    you: Option<Color>,
    /// The moves Tengen has been told about, to send only what's new.
    sent: Option<Record>,
    phase: Option<Phase>,
    dead: Vec<Point>,
    ended: bool,
    done_id: Option<i64>,
}

#[derive(Debug, Default)]
struct Tree {
    root: Option<i64>,
    active: Option<i64>,
    nodes: HashMap<i64, Node>,
}

#[derive(Debug, Default, Clone)]
struct Node {
    parent: Option<i64>,
    children: Vec<i64>,
    props: Vec<Value>,
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles one batch of downstream messages, in order.
    pub fn handle(&mut self, messages: &[Value]) -> Output {
        let mut out = Output::default();
        for m in messages {
            self.handle_one(m, &mut out);
        }
        out
    }

    fn handle_one(&mut self, m: &Value, out: &mut Output) {
        let kind = m["type"].as_str().unwrap_or("");
        let channel = m["channelId"].as_u64();
        match kind {
            "LOGIN_SUCCESS" => {
                let name = m["you"]["name"].as_str().unwrap_or("").to_string();
                let rank = m["you"]["rank"].as_str().map(str::to_string);
                self.me = Some(name.clone());
                self.my_rank = rank.clone();
                out.events.push(LiveEvent::Ready {
                    account: Account {
                        server: Server::Kgs,
                        id: name.clone(),
                        username: name,
                        rank,
                    },
                });
            }
            "AUTOMATCH_PREFS" => {
                let mut prefs = m.as_object().cloned().unwrap_or_default();
                prefs.remove("type");
                prefs.remove("channelId");
                self.prefs = Some(prefs);
            }
            "AUTOMATCH_STATUS" => {
                let enabled = m["enabled"].as_bool().unwrap_or(false);
                if !enabled && self.searching {
                    self.searching = false;
                    out.events.push(LiveEvent::SearchStopped);
                }
            }
            "GAME_JOIN" => {
                if let Some(id) = channel {
                    self.game_join(id, m, out);
                }
            }
            "GAME_UPDATE" => {
                if let Some(g) = channel.and_then(|id| self.games.get_mut(&id)) {
                    g.tree.apply(&m["sgfEvents"]);
                    report_record(channel.unwrap(), g, out);
                }
            }
            "GAME_STATE" => {
                if let Some(id) = channel {
                    if let Some(g) = self.games.get_mut(&id) {
                        game_state(id, g, m, out);
                    }
                }
            }
            "GAME_OVER" => {
                if let Some(g) = channel.and_then(|id| self.games.get_mut(&id)) {
                    if let Some(result) = result_from_score(&m["score"]) {
                        end(channel.unwrap(), g, result, out);
                    }
                }
            }
            "GAME_TIME_EXPIRED" => {
                // A check that we're still here; KGS wants the same message back.
                if let Some(id) = channel {
                    out.replies
                        .push(json!({ "type": "GAME_TIME_EXPIRED", "channelId": id }));
                }
            }
            "GAME_ALL_PLAYERS_GONE" => {}
            "UNJOIN" => {
                if let Some(id) = channel {
                    self.games.remove(&id);
                }
            }
            "IDLE_WARNING" => out.events.push(LiveEvent::Notice(
                "KGS will sign you out in 10 minutes unless you do something".into(),
            )),
            "RECONNECT" => out.events.push(LiveEvent::Disconnected {
                reason: "You signed in to KGS somewhere else".into(),
                retry: false,
            }),
            "LOGOUT" if !self.logging_out => {
                let reason = m["text"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .unwrap_or("KGS closed the connection");
                out.events.push(LiveEvent::Disconnected {
                    reason: reason.to_string(),
                    retry: true,
                });
            }
            _ => {}
        }
    }

    fn game_join(&mut self, id: u64, m: &Value, out: &mut Output) {
        let players = if m["players"].is_object() {
            &m["players"]
        } else {
            &m["gameSummary"]["players"]
        };
        let person = |role: &str| {
            let p = &players[role];
            p["name"]
                .as_str()
                .map(|n| (n.to_string(), p["rank"].as_str().map(str::to_string)))
        };
        let (black, white) = (person("black"), person("white"));
        let me = self.me.as_deref().unwrap_or("");
        let is = |p: &Option<(String, Option<String>)>| {
            p.as_ref().is_some_and(|(n, _)| n.eq_ignore_ascii_case(me))
        };
        let you = if is(&black) {
            Some(Color::Black)
        } else if is(&white) {
            Some(Color::White)
        } else {
            None
        };
        // Tengen only follows games you play; other joins are KGS bookkeeping.
        if you.is_none() {
            return;
        }
        if self.searching {
            self.searching = false;
            out.events.push(LiveEvent::SearchStopped);
        }
        let mut g = GameState {
            black,
            white,
            you,
            ..Default::default()
        };
        g.tree.apply(&m["sgfEvents"]);
        report_record(id, &mut g, out);
        game_state(id, &mut g, m, out);
        self.games.insert(id, g);
    }

    /// The message that starts automatch with the given pace.
    pub fn automatch(&mut self, size: u8, speed: Speed) -> Result<Value, String> {
        if size != 19 {
            return Err("KGS automatch plays 19 × 19 only".into());
        }
        let mut prefs = self.prefs.clone().unwrap_or_default();
        if !prefs.keys().any(|k| k.ends_with("Rank")) {
            // KGS asks unranked players how strong they are; 1k is the limit.
            let rank = self
                .my_rank
                .as_deref()
                .map(|r| r.trim_end_matches(['?', '*']).to_string());
            match rank {
                Some(r) if r.ends_with('d') || r.ends_with('p') => return Err(
                    "KGS automatch is for kyu players. Challenge someone in a KGS room instead."
                        .into(),
                ),
                Some(r) if r.ends_with('k') => prefs.insert("estimatedRank".into(), json!(r)),
                _ => prefs.insert("estimatedRank".into(), json!("20k")),
            };
        }
        prefs.entry("maxHandicap").or_insert(json!(9));
        for (key, on) in [
            ("blitzOk", speed == Speed::Blitz),
            ("fastOk", speed == Speed::Rapid),
            (
                "mediumOk",
                speed == Speed::Live || speed == Speed::Correspondence,
            ),
            ("freeOk", true),
            ("rankedOk", true),
            ("humanOk", true),
            ("robotOk", false),
            ("unrankedOk", true),
        ] {
            prefs.insert(key.into(), json!(on));
        }
        prefs.insert("type".into(), json!("AUTOMATCH_CREATE"));
        self.searching = true;
        Ok(Value::Object(prefs))
    }

    /// The `doneId` KGS needs with GAME_SCORING_DONE.
    pub fn done_id(&self, game: u64) -> Option<i64> {
        self.games.get(&game).and_then(|g| g.done_id)
    }

    /// Whether the group at `point` is marked dead right now.
    pub fn is_dead(&self, game: u64, point: Point) -> bool {
        self.games
            .get(&game)
            .is_some_and(|g| g.dead.contains(&point))
    }
}

/// Upstream message builders.
pub mod send {
    use super::*;

    pub fn login(name: &str, password: &str) -> Value {
        json!({ "type": "LOGIN", "name": name, "password": password, "locale": "en_US" })
    }

    pub fn logout() -> Value {
        json!({ "type": "LOGOUT" })
    }

    pub fn automatch_cancel() -> Value {
        json!({ "type": "AUTOMATCH_CANCEL" })
    }

    pub fn play(game: u64, mv: Move) -> Value {
        let loc = match mv {
            Move::Pass => json!("PASS"),
            Move::Play { point } => json!({ "x": point.x, "y": point.y }),
        };
        json!({ "type": "GAME_MOVE", "channelId": game, "loc": loc })
    }

    pub fn resign(game: u64) -> Value {
        json!({ "type": "GAME_RESIGN", "channelId": game })
    }

    pub fn mark_life(game: u64, point: Point, alive: bool) -> Value {
        json!({ "type": "GAME_MARK_LIFE", "channelId": game, "x": point.x, "y": point.y, "alive": alive })
    }

    pub fn scoring_done(game: u64, done_id: i64) -> Value {
        json!({ "type": "GAME_SCORING_DONE", "channelId": game, "doneId": done_id })
    }

    pub fn leave(game: u64) -> Value {
        json!({ "type": "UNJOIN_REQUEST", "channelId": game })
    }
}

fn game_state(id: u64, g: &mut GameState, m: &Value, out: &mut Output) {
    if let Some(done) = m["doneId"].as_i64() {
        g.done_id = Some(done);
    }
    if let Some(clock) = clock(id, &g.tree, &m["clocks"]) {
        out.events.push(LiveEvent::Clock(clock));
    }
    let over = m["over"].as_bool().unwrap_or(false) || !m["score"].is_null();
    let phase = if over {
        Phase::Finished
    } else if actions(m).iter().any(|a| a == "SCORE") {
        Phase::Scoring
    } else if m["actions"].is_array() {
        Phase::Play
    } else {
        return;
    };
    if g.phase != Some(phase) && phase != Phase::Finished {
        g.phase = Some(phase);
        out.events.push(LiveEvent::Phase { game_id: id, phase });
    }
    if over {
        if let Some(result) = result_from_score(&m["score"]) {
            end(id, g, result, out);
        }
    }
}

fn actions(m: &Value) -> Vec<String> {
    m["actions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["action"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn end(id: u64, g: &mut GameState, result: String, out: &mut Output) {
    if g.ended {
        return;
    }
    g.ended = true;
    g.phase = Some(Phase::Finished);
    out.events.push(LiveEvent::Phase {
        game_id: id,
        phase: Phase::Finished,
    });
    out.events.push(LiveEvent::Ended {
        game_id: id,
        result,
    });
}

/// Sends the whole game when it's new or changed, or just the next move.
fn report_record(id: u64, g: &mut GameState, out: &mut Output) {
    let (handicap, moves) = g.tree.record();
    let dead = g.tree.dead();
    if dead != g.dead {
        g.dead = dead.clone();
        out.events.push(LiveEvent::Dead {
            game_id: id,
            points: dead,
        });
    }
    if let Some((h, sent)) = &g.sent {
        if *h == handicap && moves.len() == sent.len() + 1 && moves[..sent.len()] == sent[..] {
            let (color, mv) = moves[sent.len()];
            out.events.push(LiveEvent::Moved {
                game_id: id,
                color,
                mv,
            });
            g.sent = Some((handicap, moves));
            return;
        }
        if *h == handicap && moves == *sent {
            return;
        }
    }
    let rules = g.tree.rules();
    let name = |p: &Option<(String, Option<String>)>| {
        p.as_ref().map(|(n, _)| n.clone()).unwrap_or_default()
    };
    let rank = |p: &Option<(String, Option<String>)>| p.as_ref().and_then(|(_, r)| r.clone());
    out.events.push(LiveEvent::Game(LiveGame {
        id,
        size: rules["size"].as_u64().unwrap_or(19) as u8,
        komi: rules["komi"].as_f64().unwrap_or(6.5) as f32,
        black: name(&g.black),
        white: name(&g.white),
        black_rank: rank(&g.black),
        white_rank: rank(&g.white),
        you: g.you,
        handicap: handicap.clone(),
        moves: moves.clone(),
    }));
    g.sent = Some((handicap, moves));
}

/// KGS clocks for each role, in the shape the interface draws.
fn clock(id: u64, tree: &Tree, clocks: &Value) -> Option<Clock> {
    if !clocks.is_object() {
        return None;
    }
    let rules = tree.rules();
    let system = rules["timeSystem"].as_str().unwrap_or("byo_yomi");
    let period_time = rules["byoYomiTime"].as_f64();
    let one = |c: &Value| -> PlayerClock {
        let time = c["time"].as_f64().unwrap_or(0.0);
        match system {
            "byo_yomi" => match c["periodsLeft"].as_u64() {
                // In overtime: `time` is what's left of the current period.
                Some(left) => PlayerClock {
                    main: 0.0,
                    periods: Some(left as u32),
                    period_time,
                    period_left: Some(time),
                    canadian: false,
                },
                None => PlayerClock {
                    main: time,
                    periods: rules["byoYomiPeriods"].as_u64().map(|p| p as u32),
                    period_time,
                    period_left: None,
                    canadian: false,
                },
            },
            "canadian" => match c["stonesLeft"].as_u64() {
                Some(stones) => PlayerClock {
                    main: 0.0,
                    periods: Some(stones as u32),
                    period_time,
                    period_left: Some(time),
                    canadian: true,
                },
                None => PlayerClock {
                    main: time,
                    periods: None,
                    period_time,
                    period_left: None,
                    canadian: true,
                },
            },
            _ => PlayerClock {
                main: time,
                ..Default::default()
            },
        }
    };
    let running = |role: &str| clocks[role]["running"].as_bool().unwrap_or(false);
    Some(Clock {
        game_id: id,
        to_move: if running("black") {
            Some(Color::Black)
        } else if running("white") {
            Some(Color::White)
        } else {
            None
        },
        black: one(&clocks["black"]),
        white: one(&clocks["white"]),
        paused: clocks["black"]["paused"].as_bool().unwrap_or(false)
            || clocks["white"]["paused"].as_bool().unwrap_or(false),
    })
}

/// KGS scores: a number (positive means Black won) or a word like `W+RESIGN`.
pub fn result_from_score(score: &Value) -> Option<String> {
    if let Some(n) = score.as_f64() {
        if n == 0.0 {
            return Some("0".into()); // jigo
        }
        let who = if n > 0.0 { "B" } else { "W" };
        return Some(format!("{who}+{}", trim_float(n.abs())));
    }
    let s = score.as_str()?;
    if let Ok(n) = s.parse::<f64>() {
        return result_from_score(&json!(n));
    }
    let (who, how) = s.split_once('+')?;
    let how = match how {
        "RESIGN" => "R",
        "TIME" => "T",
        "FORFEIT" => "F",
        _ => return None,
    };
    Some(format!("{who}+{how}"))
}

fn trim_float(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

impl Tree {
    fn apply(&mut self, events: &Value) {
        let Some(events) = events.as_array() else {
            return;
        };
        for e in events {
            let Some(node) = e["nodeId"].as_i64() else {
                continue;
            };
            if self.root.is_none() {
                self.root = Some(node);
            }
            let entry = self.nodes.entry(node).or_default();
            match e["type"].as_str().unwrap_or("") {
                "PROP_ADDED" => entry.props.push(e["prop"].clone()),
                "PROP_GROUP_ADDED" => {
                    if let Some(props) = e["props"].as_array() {
                        entry.props.extend(props.iter().cloned());
                    }
                }
                "PROP_REMOVED" => remove_prop(&mut entry.props, &e["prop"]),
                "PROP_GROUP_REMOVED" => {
                    if let Some(props) = e["props"].as_array() {
                        for p in props {
                            remove_prop(&mut entry.props, p);
                        }
                    }
                }
                "PROP_CHANGED" => {
                    let p = e["prop"].clone();
                    entry.props.retain(|q| !same_prop(q, &p));
                    entry.props.push(p);
                }
                "CHILD_ADDED" => {
                    if let Some(child) = e["childNodeId"].as_i64() {
                        let pos = (e["position"].as_u64().unwrap_or(0) as usize)
                            .min(entry.children.len());
                        if !entry.children.contains(&child) {
                            entry.children.insert(pos, child);
                        }
                        self.nodes.entry(child).or_default().parent = Some(node);
                    }
                }
                "CHILDREN_REORDERED" => {
                    if let Some(children) = e["children"].as_array() {
                        entry.children = children.iter().filter_map(Value::as_i64).collect();
                    }
                }
                "ACTIVATED" => self.active = Some(node),
                _ => {}
            }
        }
    }

    /// The nodes from the root to the current position.
    fn line(&self) -> Vec<i64> {
        let Some(root) = self.root else {
            return Vec::new();
        };
        if let Some(active) = self.active.filter(|a| self.nodes.contains_key(a)) {
            let mut line = vec![active];
            let mut at = active;
            while let Some(parent) = self.nodes.get(&at).and_then(|n| n.parent) {
                line.push(parent);
                at = parent;
                if line.len() > 2000 {
                    break;
                }
            }
            line.reverse();
            if line[0] == root {
                return line;
            }
        }
        let mut line = vec![root];
        let mut at = root;
        while let Some(&child) = self.nodes.get(&at).and_then(|n| n.children.first()) {
            line.push(child);
            at = child;
            if line.len() > 2000 {
                break;
            }
        }
        line
    }

    fn props(&self, node: i64) -> &[Value] {
        self.nodes
            .get(&node)
            .map(|n| n.props.as_slice())
            .unwrap_or(&[])
    }

    fn rules(&self) -> Value {
        self.root
            .and_then(|r| self.props(r).iter().find(|p| p["name"] == "RULES").cloned())
            .unwrap_or(Value::Null)
    }

    /// Handicap stones and moves along the current line.
    fn record(&self) -> Record {
        let mut handicap = Vec::new();
        let mut moves = Vec::new();
        for (i, node) in self.line().into_iter().enumerate() {
            for p in self.props(node) {
                match p["name"].as_str() {
                    Some("ADDSTONE") if i == 0 && p["color"] == "black" => {
                        handicap.extend(loc(&p["loc"]))
                    }
                    Some("MOVE") => {
                        let color = if p["color"] == "white" {
                            Color::White
                        } else {
                            Color::Black
                        };
                        let mv = match loc(&p["loc"]) {
                            Some(point) => Move::Play { point },
                            None => Move::Pass,
                        };
                        moves.push((color, mv));
                    }
                    _ => {}
                }
            }
        }
        (handicap, moves)
    }

    /// Stones marked dead at the current position.
    fn dead(&self) -> Vec<Point> {
        let Some(&last) = self.line().last() else {
            return Vec::new();
        };
        let mut points: Vec<Point> = self
            .props(last)
            .iter()
            .filter(|p| p["name"] == "DEAD")
            .filter_map(|p| loc(&p["loc"]))
            .collect();
        points.sort_by_key(|p| (p.y, p.x));
        points.dedup();
        points
    }
}

fn loc(v: &Value) -> Option<Point> {
    Some(Point::new(v["x"].as_u64()? as u8, v["y"].as_u64()? as u8))
}

fn same_prop(a: &Value, b: &Value) -> bool {
    a["name"] == b["name"]
        && a["loc"] == b["loc"]
        && (a["loc"].is_null() == b["loc"].is_null())
        && (a["name"] != "MOVE" && a["name"] != "ADDSTONE" || a["color"] == b["color"])
}

fn remove_prop(props: &mut Vec<Value>, p: &Value) {
    if let Some(i) = props.iter().position(|q| same_prop(q, p)) {
        props.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> Translator {
        let mut t = Translator::new();
        let out = t.handle(&[json!({ "type": "LOGIN_SUCCESS", "you": { "name": "rafael", "rank": "12k", "flags": "c" } })]);
        assert!(
            matches!(&out.events[0], LiveEvent::Ready { account } if account.username == "rafael" && account.rank.as_deref() == Some("12k"))
        );
        t
    }

    fn join() -> Value {
        json!({
            "type": "GAME_JOIN", "channelId": 77, "gameType": "ranked",
            "players": { "black": { "name": "Rafael", "rank": "12k" }, "white": { "name": "kumo", "rank": "10k" } },
            "sgfEvents": [
                { "type": "PROP_GROUP_ADDED", "nodeId": 1, "props": [
                    { "name": "RULES", "size": 19, "komi": 6.5, "timeSystem": "byo_yomi", "mainTime": 600, "byoYomiTime": 30, "byoYomiPeriods": 5 },
                    { "name": "PLAYERNAME", "color": "black", "text": "Rafael" } ] },
                { "type": "CHILD_ADDED", "nodeId": 1, "childNodeId": 2 },
                { "type": "PROP_ADDED", "nodeId": 2, "prop": { "name": "MOVE", "color": "black", "loc": { "x": 15, "y": 3 } } },
                { "type": "ACTIVATED", "nodeId": 2, "prevNodeId": 1 }
            ],
            "actions": [{ "action": "MOVE", "user": { "name": "kumo" } }],
            "clocks": {
                "black": { "time": 590.5 },
                "white": { "time": 600, "running": true }
            }
        })
    }

    #[test]
    fn joining_your_game_opens_it_with_your_color_and_clock() {
        let mut t = login();
        t.searching = true;
        let out = t.handle(&[join()]);
        assert_eq!(out.events[0], LiveEvent::SearchStopped);
        let LiveEvent::Game(g) = &out.events[1] else {
            panic!("{:?}", out.events)
        };
        assert_eq!(
            (g.id, g.size, g.komi, g.you),
            (77, 19, 6.5, Some(Color::Black))
        );
        assert_eq!(
            (g.black.as_str(), g.white_rank.as_deref()),
            ("Rafael", Some("10k"))
        );
        assert_eq!(
            g.moves,
            vec![(
                Color::Black,
                Move::Play {
                    point: Point::new(15, 3)
                }
            )]
        );
        let clock = out
            .events
            .iter()
            .find_map(|e| match e {
                LiveEvent::Clock(c) => Some(c),
                _ => None,
            })
            .unwrap();
        assert_eq!(clock.to_move, Some(Color::White));
        assert_eq!(
            clock.black,
            PlayerClock {
                main: 590.5,
                periods: Some(5),
                period_time: Some(30.0),
                period_left: None,
                canadian: false
            }
        );
        assert!(out.events.contains(&LiveEvent::Phase {
            game_id: 77,
            phase: Phase::Play
        }));
    }

    #[test]
    fn games_you_dont_play_are_ignored() {
        let mut t = login();
        let mut other = join();
        other["players"]["black"]["name"] = json!("someone");
        assert!(t.handle(&[other]).events.is_empty());
    }

    #[test]
    fn a_new_move_is_reported_alone_and_a_pass_too() {
        let mut t = login();
        t.handle(&[join()]);
        let out = t.handle(&[json!({ "type": "GAME_UPDATE", "channelId": 77, "sgfEvents": [
            { "type": "CHILD_ADDED", "nodeId": 2, "childNodeId": 3 },
            { "type": "PROP_ADDED", "nodeId": 3, "prop": { "name": "MOVE", "color": "white", "loc": { "x": 3, "y": 15 } } },
            { "type": "ACTIVATED", "nodeId": 3, "prevNodeId": 2 }
        ]})]);
        assert_eq!(
            out.events,
            vec![LiveEvent::Moved {
                game_id: 77,
                color: Color::White,
                mv: Move::Play {
                    point: Point::new(3, 15)
                }
            }]
        );
        let out = t.handle(&[json!({ "type": "GAME_UPDATE", "channelId": 77, "sgfEvents": [
            { "type": "CHILD_ADDED", "nodeId": 3, "childNodeId": 4 },
            { "type": "PROP_ADDED", "nodeId": 4, "prop": { "name": "MOVE", "color": "black", "loc": "PASS" } },
            { "type": "ACTIVATED", "nodeId": 4, "prevNodeId": 3 }
        ]})]);
        assert_eq!(
            out.events,
            vec![LiveEvent::Moved {
                game_id: 77,
                color: Color::Black,
                mv: Move::Pass
            }]
        );
    }

    #[test]
    fn an_undo_resends_the_whole_game() {
        let mut t = login();
        t.handle(&[join()]);
        let out = t.handle(&[
            json!({ "type": "GAME_UPDATE", "channelId": 77, "sgfEvents": [
                { "type": "ACTIVATED", "nodeId": 1, "prevNodeId": 2 }
            ]}),
        ]);
        let LiveEvent::Game(g) = &out.events[0] else {
            panic!("{:?}", out.events)
        };
        assert!(g.moves.is_empty());
    }

    #[test]
    fn handicap_stones_come_from_the_root() {
        let mut t = login();
        let mut m = join();
        m["sgfEvents"][0]["props"].as_array_mut().unwrap().extend([
            json!({ "name": "ADDSTONE", "color": "black", "loc": { "x": 15, "y": 3 } }),
            json!({ "name": "ADDSTONE", "color": "black", "loc": { "x": 3, "y": 15 } }),
        ]);
        let out = t.handle(&[m]);
        let LiveEvent::Game(g) = &out.events[0] else {
            panic!()
        };
        assert_eq!(g.handicap, vec![Point::new(15, 3), Point::new(3, 15)]);
    }

    #[test]
    fn scoring_reports_dead_stones_then_the_result() {
        let mut t = login();
        t.handle(&[join()]);
        let out = t.handle(&[
            json!({ "type": "GAME_STATE", "channelId": 77, "doneId": 4,
                    "actions": [{ "action": "SCORE", "user": { "name": "rafael" } }, { "action": "SCORE", "user": { "name": "kumo" } }] }),
            json!({ "type": "GAME_UPDATE", "channelId": 77, "sgfEvents": [
                { "type": "PROP_ADDED", "nodeId": 2, "prop": { "name": "DEAD", "loc": { "x": 15, "y": 3 } } } ] }),
        ]);
        assert_eq!(
            out.events[0],
            LiveEvent::Phase {
                game_id: 77,
                phase: Phase::Scoring
            }
        );
        assert_eq!(
            out.events[1],
            LiveEvent::Dead {
                game_id: 77,
                points: vec![Point::new(15, 3)]
            }
        );
        assert_eq!(t.done_id(77), Some(4));
        assert!(t.is_dead(77, Point::new(15, 3)));
        let out = t.handle(&[json!({ "type": "GAME_STATE", "channelId": 77, "over": true, "score": -12.5, "actions": [] })]);
        assert_eq!(
            out.events,
            vec![
                LiveEvent::Phase {
                    game_id: 77,
                    phase: Phase::Finished
                },
                LiveEvent::Ended {
                    game_id: 77,
                    result: "W+12.5".into()
                },
            ]
        );
        // GAME_OVER after the state doesn't announce it twice.
        assert!(t
            .handle(&[json!({ "type": "GAME_OVER", "channelId": 77, "score": -12.5 })])
            .events
            .is_empty());
    }

    #[test]
    fn scores_become_sgf_results() {
        assert_eq!(
            result_from_score(&json!("B+RESIGN")).as_deref(),
            Some("B+R")
        );
        assert_eq!(result_from_score(&json!("W+TIME")).as_deref(), Some("W+T"));
        assert_eq!(result_from_score(&json!(3.5)).as_deref(), Some("B+3.5"));
        assert_eq!(result_from_score(&json!(-7.0)).as_deref(), Some("W+7"));
        assert_eq!(result_from_score(&json!("UNFINISHED")), None);
    }

    #[test]
    fn connection_checks_are_answered() {
        let mut t = login();
        let out = t.handle(&[json!({ "type": "GAME_TIME_EXPIRED", "channelId": 77 })]);
        assert_eq!(
            out.replies,
            vec![json!({ "type": "GAME_TIME_EXPIRED", "channelId": 77 })]
        );
    }

    #[test]
    fn overtime_clocks_use_the_period() {
        let mut t = login();
        t.handle(&[join()]);
        let out = t.handle(&[json!({ "type": "GAME_STATE", "channelId": 77, "actions": [{ "action": "MOVE" }],
            "clocks": { "black": { "time": 21.0, "periodsLeft": 3, "running": true }, "white": { "time": 400 } } })]);
        let LiveEvent::Clock(c) = &out.events[0] else {
            panic!("{:?}", out.events)
        };
        assert_eq!(
            c.black,
            PlayerClock {
                main: 0.0,
                periods: Some(3),
                period_time: Some(30.0),
                period_left: Some(21.0),
                canadian: false
            }
        );
        assert_eq!(c.to_move, Some(Color::Black));
    }

    #[test]
    fn automatch_uses_the_saved_prefs_and_the_chosen_pace() {
        let mut t = login();
        t.handle(&[json!({ "type": "AUTOMATCH_PREFS", "maxHandicap": 6, "estiamtedRank": "14k", "blitzOk": true, "fastOk": false })]);
        let m = t.automatch(19, Speed::Rapid).unwrap();
        assert_eq!(m["type"], "AUTOMATCH_CREATE");
        assert_eq!(
            (
                m["blitzOk"].as_bool(),
                m["fastOk"].as_bool(),
                m["mediumOk"].as_bool()
            ),
            (Some(false), Some(true), Some(false))
        );
        assert_eq!(m["estiamtedRank"], "14k");
        assert_eq!(m["maxHandicap"], 6);
        assert!(t.searching);
        assert!(t.automatch(9, Speed::Blitz).is_err());
        let out = t.handle(&[json!({ "type": "AUTOMATCH_STATUS", "enabled": false })]);
        assert_eq!(out.events, vec![LiveEvent::SearchStopped]);
    }

    #[test]
    fn automatch_without_prefs_uses_your_rank() {
        let mut t = login();
        let m = t.automatch(19, Speed::Blitz).unwrap();
        assert_eq!(m["estimatedRank"], "12k");
        let mut dan = Translator::new();
        dan.handle(&[json!({ "type": "LOGIN_SUCCESS", "you": { "name": "x", "rank": "2d" } })]);
        assert!(dan.automatch(19, Speed::Blitz).unwrap_err().contains("kyu"));
    }

    #[test]
    fn being_signed_in_elsewhere_stops_reconnecting() {
        let mut t = login();
        let out = t.handle(&[json!({ "type": "RECONNECT" })]);
        assert!(matches!(
            &out.events[0],
            LiveEvent::Disconnected { retry: false, .. }
        ));
        t.logging_out = true;
        assert!(t.handle(&[json!({ "type": "LOGOUT" })]).events.is_empty());
    }

    #[test]
    fn moves_are_sent_as_kgs_expects() {
        assert_eq!(send::play(5, Move::Pass)["loc"], "PASS");
        assert_eq!(
            send::play(
                5,
                Move::Play {
                    point: Point::new(3, 4)
                }
            )["loc"],
            json!({ "x": 3, "y": 4 })
        );
        assert_eq!(
            send::scoring_done(5, 9),
            json!({ "type": "GAME_SCORING_DONE", "channelId": 5, "doneId": 9 })
        );
    }
}
