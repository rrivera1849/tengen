//! Picking the lessons in a game and building the fact sheet for each.
//!
//! Everything here is plain computation over KataGo's results and Tengen's
//! own board: no network, no LLM. A move's *point loss* is the score lead the
//! best move would have kept minus the lead after the move played, from the
//! mover's side.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use tengen_core::{Board, Color, Game, Move, Point};

use crate::analysis::GameAnalysis;
use crate::katago::{from_gtp, to_gtp, TurnResult};

const YOUR_LESSONS: usize = 5;
const THEIR_LESSONS: usize = 3;
/// Mistakes this close (in moves and on the board) are one episode.
const EPISODE_MOVES: usize = 6;
const EPISODE_DISTANCE: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A group lived or died because of the move.
    LifeAndDeath,
    /// The opponent's last move needed an answer and didn't get one.
    Urgency,
    /// Answered locally when a bigger move elsewhere was available.
    Tempo,
    /// The right area of the board, missed.
    Direction,
    /// Late, small and local.
    Endgame,
    /// A local fight or shape mistake.
    Local,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::LifeAndDeath => "Life and death",
            Kind::Urgency => "Urgent move missed",
            Kind::Tempo => "Tempo",
            Kind::Direction => "Direction of play",
            Kind::Endgame => "Endgame",
            Kind::Local => "Local fight",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    You,
    Opponent,
}

/// The smallest loss worth a lesson, by the mover's rank.
pub fn threshold(rank: Option<&str>) -> f64 {
    let r = rank.unwrap_or("").trim().to_ascii_lowercase();
    let n: u32 = r.trim_end_matches(|c: char| c.is_alphabetic() || c == '?').parse().unwrap_or(0);
    if r.ends_with('d') || r.ends_with('p') {
        2.0
    } else if r.ends_with('k') && n >= 10 {
        5.0
    } else {
        3.0
    }
}

/// Moves worth a deep look: big enough losses in positions not yet decided.
pub fn candidates(game: &Game, quick: &[TurnResult]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, (color, _)) in game.moves().iter().enumerate() {
        let (Some(before), Some(after)) = (quick.get(i), quick.get(i + 1)) else { break };
        let loss = loss_for(*color, before, after);
        let chance = chance_for(*color, before);
        let limit = threshold(rank_of(game, *color)) * 0.8; // quick pass is noisy
        if loss >= limit && ((0.1..=0.9).contains(&chance) || loss >= 12.0) {
            out.push(i);
        }
    }
    out
}

/// The turns a deep pass needs for these candidates: before and after each move.
pub fn deep_turns(candidates: &[usize]) -> Vec<usize> {
    let set: BTreeSet<usize> = candidates.iter().flat_map(|&i| [i, i + 1]).collect();
    set.into_iter().collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub side: Side,
    /// Index into the game's moves; the move number shown is this plus one.
    pub index: usize,
    pub color: Color,
    pub kind: Kind,
    pub points_lost: f64,
    pub impact: f64,
    pub played: Option<Point>,
    pub best: Option<Point>,
    /// KataGo's better line from the position before the move, mover first.
    pub line: Vec<Point>,
    /// For an opponent's mistake: your best answer's line, from the position
    /// right after their move, your color first.
    pub answer_line: Vec<Point>,
    /// For an opponent's mistake: what you actually answered.
    pub your_reply: Option<Point>,
    pub facts: FactSheet,
}

/// Picks the lessons for `you`, using the deep results in `analysis`.
pub fn episodes(game: &Game, analysis: &GameAnalysis, you: Color, candidates: &[usize]) -> Vec<Episode> {
    let size = game.info.size;
    let total = game.moves().len();
    let mut found: Vec<Episode> = Vec::new();
    for &i in candidates {
        let (Some(before), Some(after)) = (analysis.deep.get(&i), analysis.deep.get(&(i + 1))) else { continue };
        let (color, mv) = game.moves()[i];
        let loss = loss_for(color, before, after);
        if loss < threshold(rank_of(game, color)) {
            continue;
        }
        let played = match mv {
            Move::Play { point } => Some(point),
            Move::Pass => None,
        };
        let best_info = before.best();
        let best = best_info.and_then(|m| from_gtp(&m.mv, size));
        let line: Vec<Point> = best_info.map(|m| m.pv.iter().filter_map(|s| from_gtp(s, size)).take(8).collect()).unwrap_or_default();
        let board_before = board_at(game, i);
        let board_after = board_at(game, i + 1);
        let flips = group_flips(&board_after, before, after, size);
        let previous = i.checked_sub(1).and_then(|j| match game.moves()[j].1 {
            Move::Play { point } => Some(point),
            Move::Pass => None,
        });
        let kind = classify(i, total, size, loss, played, best, previous, !flips.is_empty());
        let chance = chance_for(color, before);
        let impact = loss * (1.0 - (chance - 0.5).abs() * 1.6).clamp(0.2, 1.0);
        let side = if color == you { Side::You } else { Side::Opponent };
        let punish = (side == Side::Opponent).then(|| punish_info(game, analysis, i, size));
        let (answer_line, your_reply) = if side == Side::Opponent {
            let after_info = analysis.best_known(i + 1).best();
            let answer = after_info.map(|m| m.pv.iter().filter_map(|s| from_gtp(s, size)).take(8).collect()).unwrap_or_default();
            let reply = game.moves().get(i + 1).and_then(|(_, mv)| match mv {
                Move::Play { point } => Some(*point),
                Move::Pass => None,
            });
            (answer, reply)
        } else {
            (Vec::new(), None)
        };
        let facts = FactSheet::build(FactInput {
            game, side, index: i, color, kind, loss, before, after, played, best, line: &line,
            board_before: &board_before, board_after: &board_after, flips: &flips, punish, you,
        });
        let ep = Episode { side, index: i, color, kind, points_lost: round1(loss), impact, played, best, line, answer_line, your_reply, facts };

        // Merge with the previous episode of the same player when close.
        if let Some(prev) = found.iter_mut().rev().find(|e| e.color == color) {
            let near = match (prev.played, ep.played) {
                (Some(a), Some(b)) => distance(a, b) <= EPISODE_DISTANCE,
                _ => false,
            };
            if near && i - prev.index <= EPISODE_MOVES {
                if ep.impact > prev.impact {
                    *prev = ep;
                }
                continue;
            }
        }
        found.push(ep);
    }

    let pick = |side: Side, n: usize| {
        let mut list: Vec<Episode> = found.iter().filter(|e| e.side == side).cloned().collect();
        list.sort_by(|a, b| b.impact.total_cmp(&a.impact));
        let mut chosen: Vec<Episode> = Vec::new();
        for e in &list {
            let same = chosen.iter().filter(|c| c.kind == e.kind).count();
            if chosen.len() < n && same < 2 {
                chosen.push(e.clone());
            }
        }
        // Fill up if variety left slots empty.
        for e in list {
            if chosen.len() >= n {
                break;
            }
            if !chosen.iter().any(|c| c.index == e.index) {
                chosen.push(e);
            }
        }
        chosen
    };
    let mut out = pick(Side::You, YOUR_LESSONS);
    out.extend(pick(Side::Opponent, THEIR_LESSONS));
    out.sort_by_key(|e| e.index);
    out
}

fn classify(
    index: usize,
    total: usize,
    size: u8,
    loss: f64,
    played: Option<Point>,
    best: Option<Point>,
    previous: Option<Point>,
    group_flipped: bool,
) -> Kind {
    if group_flipped {
        return Kind::LifeAndDeath;
    }
    let (Some(p), Some(b)) = (played, best) else { return Kind::Local };
    let far = distance(p, b) >= 6;
    if let Some(prev) = previous {
        if far && distance(b, prev) <= 2 && distance(p, prev) > 2 {
            return Kind::Urgency;
        }
        if far && distance(p, prev) <= 2 {
            return Kind::Tempo;
        }
    }
    if phase(index, total, size) == "endgame" && loss < 8.0 {
        return Kind::Endgame;
    }
    if far {
        Kind::Direction
    } else {
        Kind::Local
    }
}

/// For an opponent's mistake: what you played next, what would have punished it.
fn punish_info(game: &Game, analysis: &GameAnalysis, i: usize, size: u8) -> Punish {
    let after = analysis.best_known(i + 1);
    let best_reply = after.best().and_then(|m| from_gtp(&m.mv, size));
    let your_reply = game.moves().get(i + 1).and_then(|(_, mv)| match mv {
        Move::Play { point } => Some(*point),
        Move::Pass => None,
    });
    let found = match (game.moves().get(i + 1), analysis.quick.get(i + 1), analysis.quick.get(i + 2)) {
        (Some((c, _)), Some(b), Some(a)) => your_reply == best_reply || loss_for(*c, b, a) < 1.0,
        _ => false,
    };
    Punish {
        your_reply: your_reply.map(|p| to_gtp(p, size)),
        best_reply: best_reply.map(|p| to_gtp(p, size)),
        found,
    }
}

// ---------- the fact sheet ----------

/// Everything the LLM may say about one episode, computed and checked by Tengen.
#[derive(Debug, Clone, Serialize)]
pub struct FactSheet {
    pub side: Side,
    pub move_number: usize,
    pub mover: &'static str,
    pub kind: &'static str,
    pub phase: &'static str,
    pub mover_rank: Option<String>,
    pub your_rank: Option<String>,
    /// The position before the move: X is Black, O is White.
    pub board: String,
    pub recent_moves: Vec<String>,
    pub played: Spot,
    pub best: Spot,
    pub better_line: Vec<String>,
    pub points_lost: f64,
    /// The mover's chance of winning, in percent.
    pub win_chance_before: u32,
    pub win_chance_after: u32,
    pub group_changes: Vec<String>,
    pub territory_swing: Vec<Swing>,
    pub groups_under_pressure: Vec<String>,
    pub punish: Option<Punish>,
    /// Filled in by the app from the maxim store.
    pub earlier_maxims: Vec<String>,
    /// Filled in by the app: "the third game in a row with a life and death lesson".
    pub recurring: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Spot {
    pub coord: String,
    pub area: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Swing {
    pub area: &'static str,
    /// Points the mover lost in this area by playing the game move, negative if gained.
    pub points: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Punish {
    pub your_reply: Option<String>,
    pub best_reply: Option<String>,
    pub found: bool,
}

struct FactInput<'a> {
    game: &'a Game,
    side: Side,
    index: usize,
    color: Color,
    kind: Kind,
    loss: f64,
    before: &'a TurnResult,
    after: &'a TurnResult,
    played: Option<Point>,
    best: Option<Point>,
    line: &'a [Point],
    board_before: &'a Board,
    board_after: &'a Board,
    flips: &'a [String],
    punish: Option<Punish>,
    you: Color,
}

impl FactSheet {
    fn build(f: FactInput) -> FactSheet {
        let size = f.game.info.size;
        let spot = |p: Option<Point>| match p {
            Some(p) => Spot { coord: to_gtp(p, size), area: area(p, size) },
            None => Spot { coord: "pass".into(), area: "nowhere (a pass)" },
        };
        let mut c = f.color;
        let better_line = f
            .line
            .iter()
            .map(|p| {
                let s = format!("{} {}", color_name(c), to_gtp(*p, size));
                c = c.opposite();
                s
            })
            .collect();
        let start = f.index.saturating_sub(6);
        let recent_moves = f.game.moves()[start..f.index]
            .iter()
            .enumerate()
            .map(|(k, (col, mv))| {
                let at = match mv {
                    Move::Play { point } => to_gtp(*point, size),
                    Move::Pass => "pass".into(),
                };
                format!("{} {} {}", start + k + 1, color_name(*col), at)
            })
            .collect();
        let rank = |c: Color| rank_of(f.game, c).map(str::to_string);
        FactSheet {
            side: f.side,
            move_number: f.index + 1,
            mover: color_name(f.color),
            kind: f.kind.label(),
            phase: phase(f.index, f.game.moves().len(), size),
            mover_rank: rank(f.color),
            your_rank: rank(f.you),
            board: ascii(f.board_before),
            recent_moves,
            played: spot(f.played),
            best: spot(f.best),
            better_line,
            points_lost: round1(f.loss),
            win_chance_before: percent(chance_for(f.color, f.before)),
            win_chance_after: percent(chance_for(f.color, f.after)),
            group_changes: f.flips.to_vec(),
            territory_swing: swings(f.before, f.after, f.color, size),
            groups_under_pressure: pressure(f.board_after, [f.played, f.best]),
            punish: f.punish,
            earlier_maxims: Vec::new(),
            recurring: None,
        }
    }

    /// Every coordinate the sheet mentions; the LLM may name no others.
    pub fn coordinates(&self) -> BTreeSet<String> {
        let json = serde_json::to_string(self).unwrap_or_default();
        let (_, board) = json.split_once("\"board\"").unwrap_or(("", ""));
        let (_, rest) = board.split_once("\"recent_moves\"").unwrap_or(("", board));
        let mut out = BTreeSet::new();
        for word in rest.split(|c: char| !c.is_ascii_alphanumeric()) {
            if is_coordinate(word) {
                out.insert(word.to_string());
            }
        }
        out
    }

    /// Every number the LLM may quote: points and percentages from the sheet.
    pub fn numbers(&self) -> Vec<f64> {
        let mut out = vec![
            self.points_lost,
            self.win_chance_before as f64,
            self.win_chance_after as f64,
            (self.win_chance_before as f64 - self.win_chance_after as f64).abs(),
            self.move_number as f64,
        ];
        out.extend(self.territory_swing.iter().map(|s| s.points.abs()));
        out
    }
}

/// A Go coordinate on boards up to 25: a column letter (no I) and a row number.
pub fn is_coordinate(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else { return false };
    let rest = chars.as_str();
    first.is_ascii_uppercase()
        && first != 'I'
        && !rest.is_empty()
        && rest.len() <= 2
        && rest.chars().all(|c| c.is_ascii_digit())
        && rest.parse::<u32>().is_ok_and(|n| (1..=25).contains(&n))
}

/// Groups whose fate differs between the better line and the game.
fn group_flips(board: &Board, best: &TurnResult, played: &TurnResult, size: u8) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (p, color) in board.stones() {
        if seen.contains(&(p.x, p.y)) {
            continue;
        }
        let group = board.group(p);
        for s in &group {
            seen.insert((s.x, s.y));
        }
        if group.len() < 3 {
            continue;
        }
        let sign = if color == Color::Black { 1.0 } else { -1.0 };
        let avg = |r: &TurnResult| {
            group.iter().filter_map(|s| r.ownership_at(*s, size)).sum::<f64>() / group.len() as f64 * sign
        };
        let status = |v: f64| if v > 0.3 { Some(true) } else if v < -0.3 { Some(false) } else { None };
        match (status(avg(best)), status(avg(played))) {
            (Some(true), Some(false)) => out.push(format!(
                "{}'s group of {} stones around {} lives in the better line but dies after the game move",
                color_name(color), group.len(), to_gtp(center(&group), size)
            )),
            (Some(false), Some(true)) => out.push(format!(
                "{}'s group of {} stones around {} dies in the better line but lives after the game move",
                color_name(color), group.len(), to_gtp(center(&group), size)
            )),
            _ => {}
        }
    }
    out.truncate(4);
    out
}

/// Where the mover lost points: ownership after the game move minus under
/// best play, summed per area of the board.
fn swings(best: &TurnResult, played: &TurnResult, mover: Color, size: u8) -> Vec<Swing> {
    let sign = if mover == Color::Black { 1.0 } else { -1.0 };
    let mut by_area: Vec<(&'static str, f64)> = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let p = Point::new(x, y);
            let (Some(b), Some(a)) = (best.ownership_at(p, size), played.ownership_at(p, size)) else { continue };
            let lost = (b - a) * sign;
            let name = area(p, size);
            match by_area.iter_mut().find(|(n, _)| *n == name) {
                Some((_, v)) => *v += lost,
                None => by_area.push((name, lost)),
            }
        }
    }
    let mut out: Vec<Swing> = by_area
        .into_iter()
        .filter(|(_, v)| v.abs() >= 1.5)
        .map(|(area, v)| Swing { area, points: round1(v) })
        .collect();
    out.sort_by(|a, b| b.points.abs().total_cmp(&a.points.abs()));
    out.truncate(4);
    out
}

/// Groups of two or more stones with two liberties or fewer, near the action.
fn pressure(board: &Board, near: [Option<Point>; 2]) -> Vec<String> {
    let size = board.size();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (p, color) in board.stones() {
        if !seen.insert((p.x, p.y)) {
            continue;
        }
        let group = board.group(p);
        for s in &group {
            seen.insert((s.x, s.y));
        }
        let libs = board.liberties(&group);
        let close = near.iter().flatten().any(|n| group.iter().any(|s| distance(*s, *n) <= 4));
        if group.len() >= 2 && libs <= 2 && close {
            out.push(format!(
                "{}'s {} stones around {} have {} {}",
                color_name(color), group.len(), to_gtp(center(&group), size), libs,
                if libs == 1 { "liberty" } else { "liberties" }
            ));
        }
    }
    out.truncate(4);
    out
}

// ---------- small helpers ----------

fn loss_for(color: Color, before: &TurnResult, after: &TurnResult) -> f64 {
    let sign = if color == Color::Black { 1.0 } else { -1.0 };
    (before.root_info.score_lead - after.root_info.score_lead) * sign
}

fn chance_for(color: Color, r: &TurnResult) -> f64 {
    if color == Color::Black {
        r.root_info.winrate
    } else {
        1.0 - r.root_info.winrate
    }
}

fn rank_of(game: &Game, color: Color) -> Option<&str> {
    match color {
        Color::Black => game.info.black_rank.as_deref(),
        Color::White => game.info.white_rank.as_deref(),
    }
}

fn board_at(game: &Game, turn: usize) -> Board {
    let mut g = game.clone();
    g.go_to(turn);
    g.board().clone()
}

fn distance(a: Point, b: Point) -> u8 {
    a.x.abs_diff(b.x).max(a.y.abs_diff(b.y))
}

fn center(group: &[Point]) -> Point {
    let n = group.len() as f64;
    let (cx, cy) = (
        group.iter().map(|p| p.x as f64).sum::<f64>() / n,
        group.iter().map(|p| p.y as f64).sum::<f64>() / n,
    );
    *group
        .iter()
        .min_by(|a, b| {
            let d = |p: &Point| (p.x as f64 - cx).powi(2) + (p.y as f64 - cy).powi(2);
            d(a).total_cmp(&d(b))
        })
        .expect("non-empty group")
}

/// One of nine areas: four corners, four sides and the center.
pub fn area(p: Point, size: u8) -> &'static str {
    let third = |v: u8| {
        let f = v as f64 / (size.max(2) - 1) as f64;
        if f < 0.33 { 0 } else if f > 0.67 { 2 } else { 1 }
    };
    match (third(p.x), third(p.y)) {
        (0, 0) => "top left corner",
        (1, 0) => "top side",
        (2, 0) => "top right corner",
        (0, 1) => "left side",
        (1, 1) => "center",
        (2, 1) => "right side",
        (0, 2) => "bottom left corner",
        (1, 2) => "bottom side",
        _ => "bottom right corner",
    }
}

fn phase(index: usize, total: usize, size: u8) -> &'static str {
    if index < 2 * size as usize {
        "opening"
    } else if index * 10 >= total * 7 {
        "endgame"
    } else {
        "middle game"
    }
}

fn ascii(board: &Board) -> String {
    let size = board.size();
    let letters = "ABCDEFGHJKLMNOPQRSTUVWXYZ";
    let mut out = String::from("   ");
    for x in 0..size as usize {
        out.push_str(&letters[x..x + 1]);
        out.push(' ');
    }
    out.push('\n');
    for y in 0..size {
        out.push_str(&format!("{:>2} ", size - y));
        for x in 0..size {
            out.push(match board.get(Point::new(x, y)) {
                Some(Color::Black) => 'X',
                Some(Color::White) => 'O',
                None => '.',
            });
            out.push(' ');
        }
        out.push('\n');
    }
    out
}

fn color_name(c: Color) -> &'static str {
    match c {
        Color::Black => "Black",
        Color::White => "White",
    }
}

fn percent(v: f64) -> u32 {
    (v * 100.0).round().clamp(0.0, 100.0) as u32
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::katago::{MoveInfo, RootInfo};

    fn result(turn: usize, lead: f64, winrate: f64, best: &str, ownership: Option<Vec<f64>>) -> TurnResult {
        TurnResult {
            turn_number: turn,
            root_info: RootInfo { winrate, score_lead: lead, visits: 100 },
            move_infos: vec![MoveInfo { mv: best.into(), visits: 100, winrate, score_lead: lead, pv: vec![best.into()], order: 0 }],
            ownership,
        }
    }

    #[test]
    fn thresholds_follow_rank() {
        assert_eq!(threshold(Some("15k")), 5.0);
        assert_eq!(threshold(Some("4k")), 3.0);
        assert_eq!(threshold(Some("2d")), 2.0);
        assert_eq!(threshold(None), 3.0);
    }

    #[test]
    fn finds_candidates_for_both_sides_and_skips_decided_games() {
        let mut g = Game::new(9, 5.5);
        for p in [(4, 4), (2, 2), (6, 6), (6, 2)] {
            g.play(Point::new(p.0, p.1)).unwrap();
        }
        // Black loses 6 at move 1; White loses 4 at move 2; White's move 4 loses 10 in a decided game.
        let quick = vec![
            result(0, 0.0, 0.5, "E5", None),
            result(1, -6.0, 0.3, "C3", None),
            result(2, -2.0, 0.4, "G7", None),
            result(3, 30.0, 0.97, "G3", None),
            result(4, 40.0, 0.99, "A1", None),
        ];
        assert_eq!(candidates(&g, &quick), vec![0, 1]);
        assert_eq!(deep_turns(&[0, 1]), vec![0, 1, 2]);
    }

    #[test]
    fn classifies_urgency_and_tempo() {
        let p = Point::new;
        // Opponent played D4; the best move answers it; you played far away.
        assert_eq!(classify(50, 200, 19, 5.0, Some(p(15, 15)), Some(p(3, 4)), Some(p(3, 3)), false), Kind::Urgency);
        // You answered next to the opponent's move while the best was elsewhere.
        assert_eq!(classify(50, 200, 19, 5.0, Some(p(3, 4)), Some(p(15, 15)), Some(p(3, 3)), false), Kind::Tempo);
        assert_eq!(classify(180, 200, 19, 3.0, Some(p(3, 4)), Some(p(4, 4)), Some(p(10, 10)), false), Kind::Endgame);
        assert_eq!(classify(50, 200, 19, 9.0, Some(p(3, 4)), Some(p(4, 4)), None, true), Kind::LifeAndDeath);
        // Late in a very short game is still the opening, never the endgame.
        assert_eq!(classify(6, 8, 9, 3.0, Some(p(3, 4)), Some(p(4, 4)), None, false), Kind::Local);
    }

    #[test]
    fn notices_a_group_that_dies() {
        let mut b = Board::new(5);
        for (x, y) in [(0, 0), (1, 0), (2, 0)] {
            b.set(Point::new(x, y), Some(Color::White));
        }
        let alive = vec![-0.9; 25];
        let mut dead = vec![-0.9; 25];
        for i in 0..3 {
            dead[i] = 0.9;
        }
        let flips = group_flips(
            &b,
            &result(0, 0.0, 0.5, "A1", Some(alive)),
            &result(1, 0.0, 0.5, "A1", Some(dead)),
            5,
        );
        assert_eq!(flips.len(), 1);
        assert!(flips[0].starts_with("White's group of 3 stones around B5 lives in the better line"));
    }

    #[test]
    fn names_areas_and_coordinates() {
        assert_eq!(area(Point::new(3, 3), 19), "top left corner");
        assert_eq!(area(Point::new(9, 9), 19), "center");
        assert_eq!(area(Point::new(15, 9), 19), "right side");
        assert!(is_coordinate("Q16") && is_coordinate("A1"));
        assert!(!is_coordinate("I5") && !is_coordinate("B") && !is_coordinate("Q99") && !is_coordinate("Black"));
    }

    #[test]
    fn draws_the_board_as_text() {
        let mut b = Board::new(3);
        b.set(Point::new(0, 0), Some(Color::Black));
        b.set(Point::new(2, 2), Some(Color::White));
        assert_eq!(ascii(&b), "   A B C \n 3 X . . \n 2 . . . \n 1 . . O \n");
    }
}
