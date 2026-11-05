//! Live play shared by the servers that connect through a generic session
//! (KGS and Pandanet). Each adapter turns its own protocol into these events
//! and accepts these commands, so the app treats every server the same.

use serde::{Deserialize, Serialize};

use crate::adapter::{Account, Speed};
use crate::board::{Color, Point};
use crate::game::Move;

/// Each player's clock, in seconds, as of the moment the update arrived.
///
/// Serialized exactly like OGS clocks, so the interface shows both the same way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerClock {
    pub main: f64,
    /// Byo-yomi periods left (for Canadian overtime: stones left in the period).
    pub periods: Option<u32>,
    pub period_time: Option<f64>,
    /// Time left in the current byo-yomi period, once main time is used up.
    pub period_left: Option<f64>,
    /// Canadian overtime: `periods` counts the stones left to play in the
    /// current period, and there is no next period.
    #[serde(default)]
    pub canadian: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clock {
    pub game_id: u64,
    pub to_move: Option<Color>,
    pub black: PlayerClock,
    pub white: PlayerClock,
    pub paused: bool,
}

/// Everything needed to put a live game on the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveGame {
    pub id: u64,
    pub size: u8,
    pub komi: f32,
    pub black: String,
    pub white: String,
    pub black_rank: Option<String>,
    pub white_rank: Option<String>,
    /// Which side the signed-in player has, if they're playing.
    pub you: Option<Color>,
    /// Handicap stones placed before the first move.
    pub handicap: Vec<Point>,
    pub moves: Vec<(Color, Move)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Play,
    /// Both passed: players mark dead stones, then agree.
    Scoring,
    Finished,
}

/// What a connected server reports.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    /// Signed in; also sent again after a reconnect.
    Ready {
        account: Account,
    },
    /// Automatch stopped looking (found a game, was cancelled, or failed).
    SearchStopped,
    /// A game opened, or its record changed in a way that isn't one new move.
    Game(LiveGame),
    /// One new move in the open game.
    Moved {
        game_id: u64,
        color: Color,
        mv: Move,
    },
    Clock(Clock),
    Phase {
        game_id: u64,
        phase: Phase,
    },
    /// The stones currently marked dead while scoring.
    Dead {
        game_id: u64,
        points: Vec<Point>,
    },
    /// The final result in SGF form, like `B+R` or `W+3.5`.
    Ended {
        game_id: u64,
        result: String,
    },
    /// A short message worth showing the player.
    Notice(String),
    /// The connection closed. `retry` is false when reconnecting would be wrong
    /// (signed in elsewhere, bad password).
    Disconnected {
        reason: String,
        retry: bool,
    },
}

/// Commands to a connected server. Each call queues the message and returns;
/// answers come back as [`LiveEvent`]s.
pub trait LiveClient: Send + Sync {
    /// Starts looking for an opponent. Errors are for settings the server
    /// can't offer (a board size it doesn't automatch, for example).
    fn find_match(&self, size: u8, speed: Speed) -> Result<(), String>;
    fn cancel_match(&self);
    fn play(&self, game_id: u64, mv: Move);
    fn resign(&self, game_id: u64);
    /// While scoring: marks the group at `point` dead, or alive again.
    fn toggle_dead(&self, game_id: u64, point: Point, dead: bool);
    /// Whether players mark dead stones themselves while scoring. Fox judges
    /// them itself, so its client says no.
    fn marks_dead_stones(&self) -> bool {
        true
    }
    /// While scoring: agrees with the stones marked dead.
    fn accept_score(&self, game_id: u64);
    /// Stops following a game.
    fn leave_game(&self, game_id: u64);
    /// Signs out and closes the connection.
    fn close(&self);
}

/// Standard handicap stones (fixed placement) for `n` stones on a `size` board.
pub fn handicap_points(size: u8, n: u8) -> Vec<Point> {
    if n < 2 || size < 7 {
        return Vec::new();
    }
    let edge = if size >= 13 { 3 } else { 2 };
    let (lo, hi, mid) = (edge, size - 1 - edge, size / 2);
    let odd = size % 2 == 1;
    // Corners first (upper right, lower left, lower right, upper left), then
    // the sides, then the center, as on most servers.
    let mut all = vec![
        Point::new(hi, lo),
        Point::new(lo, hi),
        Point::new(hi, hi),
        Point::new(lo, lo),
    ];
    if odd {
        let sides_lr = [Point::new(lo, mid), Point::new(hi, mid)];
        let sides_tb = [Point::new(mid, lo), Point::new(mid, hi)];
        let center = Point::new(mid, mid);
        let n = n.min(9) as usize;
        let mut out: Vec<Point> = all.iter().take(n.min(4)).copied().collect();
        match n {
            5 => out.push(center),
            6 => out.extend(sides_lr),
            7 => {
                out.extend(sides_lr);
                out.push(center);
            }
            8 => {
                out.extend(sides_lr);
                out.extend(sides_tb);
            }
            9 => {
                out.extend(sides_lr);
                out.extend(sides_tb);
                out.push(center);
            }
            _ => {}
        }
        out
    } else {
        all.truncate(n.min(4) as usize);
        all
    }
}

/// `D4`-style coordinates (no I), as used by IGS and GTP, to a point. Row 1 is
/// the bottom edge.
pub fn parse_vertex(s: &str, size: u8) -> Option<Point> {
    let s = s.trim();
    let mut chars = s.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() || letter == 'I' {
        return None;
    }
    let mut col = letter as u8 - b'A';
    if letter > 'I' {
        col -= 1;
    }
    let row: u8 = chars.as_str().parse().ok()?;
    if col >= size || row == 0 || row > size {
        return None;
    }
    Some(Point::new(col, size - row))
}

pub fn vertex(p: Point, size: u8) -> String {
    let mut letter = b'A' + p.x;
    if letter >= b'I' {
        letter += 1;
    }
    format!("{}{}", letter as char, size - p.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertices_round_trip_and_skip_i() {
        assert_eq!(parse_vertex("D4", 19), Some(Point::new(3, 15)));
        assert_eq!(parse_vertex("J10", 19), Some(Point::new(8, 9)));
        assert_eq!(parse_vertex("T19", 19), Some(Point::new(18, 0)));
        assert_eq!(parse_vertex("I5", 19), None);
        assert_eq!(parse_vertex("Z1", 19), None);
        for p in [Point::new(0, 0), Point::new(8, 3), Point::new(18, 18)] {
            assert_eq!(parse_vertex(&vertex(p, 19), 19), Some(p));
        }
    }

    #[test]
    fn handicap_stones_are_star_points() {
        assert_eq!(
            handicap_points(19, 2),
            vec![Point::new(15, 3), Point::new(3, 15)]
        );
        let nine = handicap_points(19, 9);
        assert_eq!(nine.len(), 9);
        assert!(nine.contains(&Point::new(9, 9)));
        assert_eq!(handicap_points(19, 5).last(), Some(&Point::new(9, 9)));
        assert_eq!(handicap_points(9, 4).len(), 4);
        assert!(handicap_points(19, 1).is_empty());
    }

    #[test]
    fn clocks_serialize_like_ogs() {
        let c = PlayerClock {
            main: 30.0,
            periods: Some(5),
            period_time: Some(30.0),
            period_left: None,
            canadian: false,
        };
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            r#"{"main":30.0,"periods":5,"periodTime":30.0,"periodLeft":null,"canadian":false}"#
        );
    }
}
