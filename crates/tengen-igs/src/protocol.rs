//! Turns IGS text lines into Tengen events, and builds the commands Tengen
//! sends. No networking here, so every rule can be tested with sample lines.
//!
//! With `toggle client true`, every line starts with a number that says what
//! it is: `15` game headers and moves, `9` information, `63` seek (automatch),
//! `49` stones removed while scoring, `5` errors, `1` the prompt.

use regex::Regex;
use std::sync::OnceLock;
use tengen_core::adapter::{Account, Server, Speed};
use tengen_core::live::{
    handicap_points, parse_vertex, vertex, Clock, LiveEvent, LiveGame, Phase, PlayerClock,
};
use tengen_core::{Board, Color, Move, Point};

/// One automatch time setting offered by the server (`seek config_list`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeekConfig {
    pub id: u32,
    pub main: u32,
    /// Canadian overtime: `stones` moves in `period` seconds.
    pub period: u32,
    pub stones: u32,
}

#[derive(Debug, Default)]
pub struct Translator {
    me: String,
    pub configs: Vec<SeekConfig>,
    pub searching: bool,
    game: Option<IgsGame>,
    /// The game a `15` header just announced; move lines that follow belong to it.
    current: Option<u64>,
}

#[derive(Debug, Default, Clone)]
struct IgsGame {
    id: u64,
    size: u8,
    komi: f32,
    black: String,
    white: String,
    black_rank: Option<String>,
    white_rank: Option<String>,
    you: Option<Color>,
    handicap: u8,
    moves: Vec<(Color, Move)>,
    /// Whether Tengen has been sent this game yet.
    announced: bool,
    phase: Phase,
    dead: Vec<Point>,
    ended: bool,
    /// Byo-yomi time from the games list, in seconds.
    period: Option<f64>,
    /// The last times from a header: (seconds, stones left or -1) for white, then black.
    times: Option<((f64, i64), (f64, i64))>,
}

/// Commands and events produced by one line.
#[derive(Debug, Default, PartialEq)]
pub struct Output {
    pub events: Vec<LiveEvent>,
    /// Commands to send right away (asking for game details, for example).
    pub commands: Vec<String>,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

fn header_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Game 43 I: white (0 600 -1) vs black (2 598 -1)
    re(
        &R,
        r"^Game (\d+) I: (\S+) \((\d+) (-?\d+) (-?\d+)\) vs (\S+) \((\d+) (-?\d+) (-?\d+)\)",
    )
}

fn move_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // 12(B): D4   or   13(W): Pass   or   0(B): Handicap 3
    re(&R, r"^(\d+)\((B|W)\): (.+)$")
}

fn games_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // [43]  white [ 3k*] vs.  black [ 4k*] (  0   19  0  6.5  10  I) (  0)
    re(
        &R,
        r"^\[\s*(\d+)\]\s+(\S+)\s+\[\s*([^\]]*?)\s*\]\s+vs\.\s+(\S+)\s+\[\s*([^\]]*?)\s*\]\s+\(\s*(\d+)\s+(\d+)\s+(\d+)\s+(-?[\d.]+)\s+(\d+)",
    )
}

fn result_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // {Game 43: white vs black : W 62.5 B 70.0}   {Game 43: white vs black : Black resigns.}
    re(&R, r"^\{Game (\d+): (\S+) vs (\S+) : (.+)\}")
}

impl Translator {
    pub fn new(me: &str) -> Self {
        Translator {
            me: me.to_string(),
            ..Default::default()
        }
    }

    pub fn account(&self) -> Account {
        let rank = self.game.as_ref().and_then(|g| match g.you {
            Some(Color::Black) => g.black_rank.clone(),
            Some(Color::White) => g.white_rank.clone(),
            None => None,
        });
        Account {
            server: Server::Igs,
            id: self.me.clone(),
            username: self.me.clone(),
            rank,
        }
    }

    /// Handles one line from the server, without its line ending.
    pub fn handle(&mut self, line: &str) -> Output {
        let mut out = Output::default();
        let line = line.trim_end_matches(['\r', '\n']);
        let (code, rest) = match line.split_once(' ') {
            Some((c, r)) if c.chars().all(|ch| ch.is_ascii_digit()) => (c, r),
            _ => return out,
        };
        match code {
            "15" => self.game_line(rest.trim(), &mut out),
            "9" | "21" => self.info(rest.trim(), &mut out),
            "7" => self.games_line(rest.trim(), &mut out),
            "63" => self.seek(rest.trim(), &mut out),
            "49" => self.removing(rest.trim(), &mut out),
            "28" => self.undo(&mut out),
            "5" => out.events.push(LiveEvent::Notice(rest.trim().to_string())),
            _ => {}
        }
        out
    }

    fn game_line(&mut self, rest: &str, out: &mut Output) {
        if let Some(c) = header_re().captures(rest) {
            let id: u64 = c[1].parse().unwrap_or(0);
            let (white, black) = (c[2].to_string(), c[6].to_string());
            self.current = Some(id);
            let you = if black.eq_ignore_ascii_case(&self.me) {
                Some(Color::Black)
            } else if white.eq_ignore_ascii_case(&self.me) {
                Some(Color::White)
            } else {
                None
            };
            if you.is_none() {
                return; // someone else's game
            }
            let new = self.game.as_ref().is_none_or(|g| g.id != id || g.ended);
            if new {
                if self.searching {
                    self.searching = false;
                    out.events.push(LiveEvent::SearchStopped);
                }
                self.game = Some(IgsGame {
                    id,
                    size: 19,
                    komi: 6.5,
                    black,
                    white,
                    you,
                    ..Default::default()
                });
                // Board size, komi and ranks come from the games list.
                out.commands.push(format!("games {id}"));
            }
            let side = |n: usize| -> (f64, i64) {
                (
                    c[n + 1].parse().unwrap_or(0.0),
                    c[n + 2].parse().unwrap_or(-1),
                )
            };
            let g = self.game.as_mut().expect("game");
            g.times = Some((side(3), side(7)));
            out.events.extend(g.clock());
            return;
        }
        let Some(c) = move_re().captures(rest) else {
            return;
        };
        let Some(g) = self.game.as_mut().filter(|g| Some(g.id) == self.current) else {
            return;
        };
        let n: usize = c[1].parse().unwrap_or(0);
        let color = if &c[2] == "B" {
            Color::Black
        } else {
            Color::White
        };
        let text = c[3].trim();
        if let Some(count) = text.strip_prefix("Handicap ") {
            g.handicap = count.trim().parse().unwrap_or(0);
            announce(g, out, true);
            return;
        }
        let mv = if text.eq_ignore_ascii_case("pass") {
            Move::Pass
        } else {
            // Stones listed after the move are the ones it captured.
            match text
                .split_whitespace()
                .next()
                .and_then(|v| parse_vertex(v, g.size))
            {
                Some(point) => Move::Play { point },
                None => return,
            }
        };
        // With a handicap, move 0 is the handicap and the first real move is 1.
        let index = if g.handicap >= 2 {
            n.saturating_sub(1)
        } else {
            n
        };
        if index == g.moves.len() {
            g.moves.push((color, mv));
            if g.announced {
                out.events.push(LiveEvent::Moved {
                    game_id: g.id,
                    color,
                    mv,
                });
            } else {
                announce(g, out, false);
            }
            // The header came before the move, so the clock that runs has changed.
            out.events.extend(g.clock());
        } else if index < g.moves.len() && g.moves[index] != (color, mv) {
            g.moves.truncate(index);
            g.moves.push((color, mv));
            announce(g, out, true);
        } else if !g.announced {
            announce(g, out, false);
        }
    }

    fn games_line(&mut self, rest: &str, out: &mut Output) {
        let Some(c) = games_re().captures(rest) else {
            return;
        };
        let Some(g) = self.game.as_mut() else { return };
        if c[1].parse::<u64>().ok() != Some(g.id) {
            return;
        }
        let rank = |s: &str| (!s.is_empty()).then(|| s.to_string());
        g.white_rank = rank(&c[3]);
        g.black_rank = rank(&c[5]);
        g.size = c[7].parse().unwrap_or(19);
        g.handicap = c[8].parse().unwrap_or(g.handicap);
        g.komi = c[9].parse().unwrap_or(g.komi);
        let minutes: f64 = c[10].parse().unwrap_or(0.0);
        g.period = (minutes > 0.0).then_some(minutes * 60.0);
        announce(g, out, true);
    }

    fn info(&mut self, rest: &str, out: &mut Output) {
        if let Some(c) = result_re().captures(rest) {
            let id: u64 = c[1].parse().unwrap_or(0);
            let Some(g) = self.game.as_mut().filter(|g| g.id == id && !g.ended) else {
                return;
            };
            if let Some(result) = parse_result(&c[4]) {
                g.ended = true;
                g.phase = Phase::Finished;
                out.events.push(LiveEvent::Phase {
                    game_id: id,
                    phase: Phase::Finished,
                });
                out.events.push(LiveEvent::Ended {
                    game_id: id,
                    result,
                });
            }
            return;
        }
        let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
            return;
        };
        if rest.contains("check your score with the score command") {
            g.phase = Phase::Scoring;
            g.dead.clear();
            out.events.push(LiveEvent::Phase {
                game_id: g.id,
                phase: Phase::Scoring,
            });
        } else if rest.contains("Board is restored to what it was when you started scoring") {
            g.dead.clear();
            out.events.push(LiveEvent::Dead {
                game_id: g.id,
                points: Vec::new(),
            });
        } else if rest.contains("has typed done") {
            out.events.push(LiveEvent::Notice(rest.to_string()));
        }
    }

    fn removing(&mut self, rest: &str, out: &mut Output) {
        // Game 43 rafael is removing @ C3
        let Some(g) = self.game.as_mut().filter(|g| g.phase == Phase::Scoring) else {
            return;
        };
        let Some(at) = rest.split("@").nth(1).and_then(|v| parse_vertex(v, g.size)) else {
            return;
        };
        let board = g.board();
        if board.get(at).is_none() {
            return;
        }
        for p in board.group(at) {
            if !g.dead.contains(&p) {
                g.dead.push(p);
            }
        }
        g.dead.sort_by_key(|p| (p.y, p.x));
        out.events.push(LiveEvent::Dead {
            game_id: g.id,
            points: g.dead.clone(),
        });
    }

    fn undo(&mut self, out: &mut Output) {
        // 28 Undo in game 43: rafael vs kumo:  D4
        let Some(g) = self.game.as_mut().filter(|g| !g.ended) else {
            return;
        };
        if g.moves.pop().is_some() {
            announce(g, out, true);
        }
    }

    fn seek(&mut self, rest: &str, out: &mut Output) {
        let words: Vec<&str> = rest.split_whitespace().collect();
        match words.first().copied() {
            Some("CONFIG_LIST_START") => self.configs.clear(),
            Some("CONFIG_LIST") => {
                let n = |i: usize| words.get(i).and_then(|w| w.parse().ok()).unwrap_or(0);
                self.configs.push(SeekConfig {
                    id: n(1),
                    main: n(2),
                    period: n(3),
                    stones: n(4),
                });
            }
            Some("OPPONENT_FOUND") | Some("ENTRY_CANCEL") => {
                if self.searching {
                    self.searching = false;
                    out.events.push(LiveEvent::SearchStopped);
                }
            }
            Some("ERROR") => {
                self.searching = false;
                out.events.push(LiveEvent::SearchStopped);
                out.events.push(LiveEvent::Notice(format!(
                    "Pandanet: {}",
                    words[1..].join(" ")
                )));
            }
            _ => {}
        }
    }

    /// The command that starts automatch, using the server's time settings.
    pub fn automatch(&mut self, size: u8, speed: Speed) -> Result<String, String> {
        if size != 19 {
            return Err("Pandanet automatch plays 19 × 19 only".into());
        }
        let mut configs = self.configs.clone();
        if configs.is_empty() {
            return Err(
                "Pandanet hasn't sent its automatch settings yet. Try again in a moment.".into(),
            );
        }
        // Shortest total time first.
        configs.sort_by_key(|c| c.main + c.period);
        let pick = match speed {
            Speed::Blitz => configs[0],
            Speed::Rapid => configs[configs.len() / 2],
            Speed::Live | Speed::Correspondence => configs[configs.len() - 1],
        };
        self.searching = true;
        // Opponents up to three ranks weaker or stronger, no handicap limit set.
        Ok(format!("seek entry {} 19 3 3 0", pick.id))
    }

    /// Whether this game is the one being played.
    pub fn game_size(&self, game: u64) -> Option<u8> {
        self.game.as_ref().filter(|g| g.id == game).map(|g| g.size)
    }
}

impl IgsGame {
    fn clock(&self) -> Option<LiveEvent> {
        let (w, b) = self.times?;
        let to_move = match self.moves.last() {
            Some((c, _)) => c.opposite(),
            None if self.handicap >= 2 => Color::White,
            None => Color::Black,
        };
        let player = |(time, stones): (f64, i64)| {
            if stones < 0 {
                PlayerClock {
                    main: time,
                    periods: None,
                    period_time: self.period,
                    period_left: None,
                    canadian: true,
                }
            } else {
                PlayerClock {
                    main: 0.0,
                    periods: Some(stones as u32),
                    period_time: self.period,
                    period_left: Some(time),
                    canadian: true,
                }
            }
        };
        Some(LiveEvent::Clock(Clock {
            game_id: self.id,
            to_move: (self.phase == Phase::Play).then_some(to_move),
            black: player(b),
            white: player(w),
            paused: false,
        }))
    }

    fn board(&self) -> Board {
        let mut b = Board::new(self.size);
        for p in handicap_points(self.size, self.handicap) {
            b.set(p, Some(Color::Black));
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

fn announce(g: &mut IgsGame, out: &mut Output, force: bool) {
    if g.announced && !force {
        return;
    }
    g.announced = true;
    out.events.push(LiveEvent::Game(LiveGame {
        id: g.id,
        size: g.size,
        komi: g.komi,
        black: g.black.clone(),
        white: g.white.clone(),
        black_rank: g.black_rank.clone(),
        white_rank: g.white_rank.clone(),
        you: g.you,
        handicap: handicap_points(g.size, g.handicap),
        moves: g.moves.clone(),
    }));
}

/// `W 62.5 B 70.0`, `Black resigns.`, `White forfeits on time.` to SGF form.
pub fn parse_result(text: &str) -> Option<String> {
    let text = text.trim();
    let winner_of = |loser: &str| if loser.starts_with("Black") { "W" } else { "B" };
    if text.contains("resign") {
        return Some(format!("{}+R", winner_of(text)));
    }
    if text.contains("forfeits on time") {
        return Some(format!("{}+T", winner_of(text)));
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if let [a, x, b, y, ..] = words[..] {
        let (x, y): (f64, f64) = (x.parse().ok()?, y.parse().ok()?);
        let (w, bl) = match (a, b) {
            ("W", "B") => (x, y),
            ("B", "W") => (y, x),
            _ => return None,
        };
        let diff = (w - bl).abs();
        let margin = if diff.fract() == 0.0 {
            format!("{diff:.0}")
        } else {
            format!("{diff}")
        };
        return Some(if diff == 0.0 {
            "0".into()
        } else if w > bl {
            format!("W+{margin}")
        } else {
            format!("B+{margin}")
        });
    }
    None
}

/// Commands Tengen sends.
pub mod send {
    use super::*;

    pub fn play(size: u8, game: u64, mv: Move) -> String {
        match mv {
            Move::Pass => "pass".into(),
            Move::Play { point } => format!("{} {game}", vertex(point, size)),
        }
    }

    /// While scoring, naming a stone removes its group.
    pub fn remove(size: u8, game: u64, point: Point) -> String {
        format!("{} {game}", vertex(point, size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Translator, lines: &[&str]) -> Output {
        let mut all = Output::default();
        for l in lines {
            let o = t.handle(l);
            all.events.extend(o.events);
            all.commands.extend(o.commands);
        }
        all
    }

    fn started() -> Translator {
        let mut t = Translator::new("rafael");
        t.searching = true;
        let out = run(
            &mut t,
            &[
                "63 OPPONENT_FOUND kumo",
                "9 Creating match [43] with kumo.",
                "15 Game 43 I: kumo (0 60 -1) vs rafael (0 60 -1)",
            ],
        );
        assert_eq!(out.events[0], LiveEvent::SearchStopped);
        assert_eq!(out.commands, vec!["games 43"]);
        t
    }

    #[test]
    fn a_game_with_you_in_it_opens_and_takes_details_from_the_games_list() {
        let mut t = started();
        let out = run(
            &mut t,
            &["7 [43]    kumo [10k*] vs.  rafael [12k*] (  0   19  0  6.5  10  I) (  0)"],
        );
        let LiveEvent::Game(g) = &out.events[0] else {
            panic!("{:?}", out.events)
        };
        assert_eq!(
            (g.id, g.size, g.komi, g.you),
            (43, 19, 6.5, Some(Color::Black))
        );
        assert_eq!(
            (
                g.white.as_str(),
                g.white_rank.as_deref(),
                g.black_rank.as_deref()
            ),
            ("kumo", Some("10k*"), Some("12k*"))
        );
        assert_eq!(t.account().rank.as_deref(), Some("12k*"));
    }

    #[test]
    fn moves_and_passes_arrive_one_at_a_time() {
        let mut t = started();
        run(
            &mut t,
            &["7 [43]    kumo [10k*] vs.  rafael [12k*] (  0   19  0  6.5  10  I) (  0)"],
        );
        let out = run(
            &mut t,
            &[
                "15 Game 43 I: kumo (0 60 -1) vs rafael (0 55 -1)",
                "15   0(B): Q16",
            ],
        );
        assert!(
            matches!(&out.events[0], LiveEvent::Clock(c) if c.to_move == Some(Color::Black) && c.black.main == 55.0)
        );
        assert_eq!(
            out.events[1],
            LiveEvent::Moved {
                game_id: 43,
                color: Color::Black,
                mv: Move::Play {
                    point: Point::new(15, 3)
                }
            }
        );
        let out = run(
            &mut t,
            &[
                "15 Game 43 I: kumo (0 58 -1) vs rafael (0 55 -1)",
                "15   1(W): Pass",
            ],
        );
        assert_eq!(
            out.events[1],
            LiveEvent::Moved {
                game_id: 43,
                color: Color::White,
                mv: Move::Pass
            }
        );
        // The full list again changes nothing.
        let out = run(
            &mut t,
            &[
                "15 Game 43 I: kumo (0 58 -1) vs rafael (0 55 -1)",
                "15   0(B): Q16",
                "15   1(W): Pass",
            ],
        );
        assert_eq!(out.events.len(), 1);
    }

    #[test]
    fn captures_listed_after_a_move_are_not_moves() {
        let mut t = started();
        let out = run(
            &mut t,
            &[
                "15 Game 43 I: kumo (1 60 -1) vs rafael (0 60 -1)",
                "15   0(B): D4 C3 C5",
            ],
        );
        // The first move opens the game; its record has D4 and nothing else.
        let LiveEvent::Game(g) = out
            .events
            .iter()
            .find(|e| matches!(e, LiveEvent::Game(_)))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            g.moves,
            vec![(
                Color::Black,
                Move::Play {
                    point: Point::new(3, 15)
                }
            )]
        );
    }

    #[test]
    fn handicap_games_place_stones_and_number_moves_from_one() {
        let mut t = started();
        let out = run(
            &mut t,
            &[
                "15 Game 43 I: kumo (0 60 -1) vs rafael (0 60 -1)",
                "15   0(B): Handicap 2",
                "15   1(W): C3",
            ],
        );
        let game = out
            .events
            .iter()
            .rev()
            .find_map(|e| match e {
                LiveEvent::Game(g) => Some(g),
                _ => None,
            })
            .unwrap();
        assert_eq!(game.handicap.len(), 2);
        assert!(out.events.contains(&LiveEvent::Moved {
            game_id: 43,
            color: Color::White,
            mv: Move::Play {
                point: Point::new(2, 16)
            }
        }));
    }

    #[test]
    fn overtime_clocks_count_stones() {
        let mut t = started();
        run(
            &mut t,
            &[
                "7 [43]    kumo [10k*] vs.  rafael [12k*] (  0   19  0  6.5  10  I) (  0)",
                "15   0(B): Q16",
            ],
        );
        let out = run(
            &mut t,
            &["15 Game 43 I: kumo (0 412 18) vs rafael (0 60 -1)"],
        );
        let LiveEvent::Clock(c) = &out.events[0] else {
            panic!()
        };
        assert_eq!(
            c.white,
            PlayerClock {
                main: 0.0,
                periods: Some(18),
                period_time: Some(600.0),
                period_left: Some(412.0),
                canadian: true
            }
        );
        assert_eq!(c.to_move, Some(Color::White));
    }

    #[test]
    fn scoring_marks_whole_groups_dead_and_can_be_reset() {
        let mut t = started();
        run(
            &mut t,
            &[
                "15 Game 43 I: kumo (0 60 -1) vs rafael (0 60 -1)",
                "15   0(B): D4",
                "15   1(W): Q16",
                "15   2(B): E4",
                "15   3(W): Pass",
                "15   4(B): Pass",
            ],
        );
        let out = run(
            &mut t,
            &["9 You can check your score with the score command, type 'done' when finished."],
        );
        assert_eq!(
            out.events,
            vec![LiveEvent::Phase {
                game_id: 43,
                phase: Phase::Scoring
            }]
        );
        let out = run(&mut t, &["49 Game 43 kumo is removing @ D4"]);
        assert_eq!(
            out.events,
            vec![LiveEvent::Dead {
                game_id: 43,
                points: vec![Point::new(3, 15), Point::new(4, 15)]
            }]
        );
        let out = run(
            &mut t,
            &["9 Board is restored to what it was when you started scoring"],
        );
        assert_eq!(
            out.events,
            vec![LiveEvent::Dead {
                game_id: 43,
                points: vec![]
            }]
        );
    }

    #[test]
    fn results_end_the_game_once() {
        let mut t = started();
        let out = run(&mut t, &["9 {Game 43: kumo vs rafael : W 62.5 B 70.0}"]);
        assert_eq!(
            out.events[1],
            LiveEvent::Ended {
                game_id: 43,
                result: "B+7.5".into()
            }
        );
        assert!(
            run(&mut t, &["21 {Game 43: kumo vs rafael : W 62.5 B 70.0}"])
                .events
                .is_empty()
        );
    }

    #[test]
    fn results_read_like_sgf() {
        assert_eq!(parse_result("Black resigns.").as_deref(), Some("W+R"));
        assert_eq!(
            parse_result("White forfeits on time.").as_deref(),
            Some("B+T")
        );
        assert_eq!(parse_result("W 75.5 B 70.0").as_deref(), Some("W+5.5"));
        assert_eq!(parse_result("B 80 W 74").as_deref(), Some("B+6"));
        assert_eq!(parse_result("has adjourned."), None);
    }

    #[test]
    fn other_peoples_games_are_ignored() {
        let mut t = Translator::new("rafael");
        let out = run(
            &mut t,
            &[
                "15 Game 7 I: a (0 60 -1) vs b (0 60 -1)",
                "15   0(B): D4",
                "21 {Game 7: a vs b : Black resigns.}",
            ],
        );
        assert!(out.events.is_empty() && out.commands.is_empty());
    }

    #[test]
    fn automatch_picks_a_time_setting_by_pace() {
        let mut t = Translator::new("rafael");
        assert!(t.automatch(19, Speed::Blitz).is_err());
        run(
            &mut t,
            &[
                "63 CONFIG_LIST_START 4",
                "63 CONFIG_LIST 0 60 600 25 0 0",
                "63 CONFIG_LIST 1 60 480 25 0 0",
                "63 CONFIG_LIST 2 60 300 25 0 0",
                "63 CONFIG_LIST 3 60 900 25 0 0",
                "63 CONFIG_LIST_END",
            ],
        );
        assert_eq!(
            t.automatch(19, Speed::Blitz).unwrap(),
            "seek entry 2 19 3 3 0"
        );
        assert_eq!(
            t.automatch(19, Speed::Live).unwrap(),
            "seek entry 3 19 3 3 0"
        );
        assert!(t.automatch(13, Speed::Live).is_err());
        let out = run(&mut t, &["63 ERROR You are already in a game"]);
        assert_eq!(out.events[0], LiveEvent::SearchStopped);
    }

    #[test]
    fn commands_use_igs_coordinates() {
        assert_eq!(
            send::play(
                19,
                43,
                Move::Play {
                    point: Point::new(3, 15)
                }
            ),
            "D4 43"
        );
        assert_eq!(send::play(19, 43, Move::Pass), "pass");
        assert_eq!(send::remove(19, 43, Point::new(8, 9)), "J10 43");
    }
}
