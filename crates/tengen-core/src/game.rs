use serde::{Deserialize, Serialize};

use crate::board::{Board, Color, MoveError, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Move {
    Play { point: Point },
    Pass,
}

/// Metadata of a game, mirroring the common SGF root properties.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GameInfo {
    pub size: u8,
    pub komi: f32,
    pub black: Option<String>,
    pub white: Option<String>,
    pub black_rank: Option<String>,
    pub white_rank: Option<String>,
    pub result: Option<String>,
    /// Day the game was played, `YYYY-MM-DD` (SGF `DT`).
    pub date: Option<String>,
    /// Local start time, `HH:MM`. SGF has no standard place for it, so it is
    /// used for filenames only and not written into the record.
    pub time: Option<String>,
    pub rules: Option<String>,
    /// Which server the game came from ("OGS", "Fox"), if any.
    pub server: Option<String>,
    pub event: Option<String>,
    /// Setup stones placed before the first move (handicap or an edited position).
    pub setup_black: Vec<Point>,
    pub setup_white: Vec<Point>,
}

/// A game record plus a cursor for stepping through it.
///
/// The cursor counts moves already applied, so `cursor == 0` is the setup
/// position and `cursor == moves.len()` is the latest position.
#[derive(Debug, Clone)]
pub struct Game {
    pub info: GameInfo,
    moves: Vec<(Color, Move)>,
    cursor: usize,
    board: Board,
}

impl Game {
    pub fn new(size: u8, komi: f32) -> Self {
        Self::from_info(GameInfo { size, komi, ..Default::default() })
    }

    pub fn from_info(info: GameInfo) -> Self {
        let board = Self::setup_board(&info);
        Game { info, moves: Vec::new(), cursor: 0, board }
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn moves(&self) -> &[(Color, Move)] {
        &self.moves
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn last_move(&self) -> Option<(Color, Move)> {
        self.cursor.checked_sub(1).map(|i| self.moves[i])
    }

    /// Whose turn it is at the cursor.
    pub fn to_move(&self) -> Color {
        match self.last_move() {
            Some((c, _)) => c.opposite(),
            None if !self.info.setup_black.is_empty() && self.info.setup_white.is_empty() => {
                Color::White
            }
            None => Color::Black,
        }
    }

    /// Plays at the cursor. Any moves after the cursor are replaced.
    pub fn play(&mut self, point: Point) -> Result<Vec<Point>, MoveError> {
        let color = self.to_move();
        self.play_as(color, Move::Play { point })
    }

    pub fn pass(&mut self) {
        let color = self.to_move();
        let _ = self.play_as(color, Move::Pass);
    }

    /// Applies a move for an explicit color (used when loading records,
    /// where the file, not the turn order, says who played).
    pub fn play_as(&mut self, color: Color, mv: Move) -> Result<Vec<Point>, MoveError> {
        let captured = match mv {
            Move::Play { point } => self.board.play(point, color)?,
            Move::Pass => {
                self.board.pass();
                Vec::new()
            }
        };
        self.moves.truncate(self.cursor);
        self.moves.push((color, mv));
        self.cursor += 1;
        Ok(captured)
    }

    /// Like [`Game::play_as`], but never refuses: an illegal move from a
    /// server record is still placed, so the record stays complete.
    pub fn play_record(&mut self, color: Color, mv: Move) {
        if self.play_as(color, mv).is_err() {
            if let Move::Play { point } = mv {
                self.board.set(point, Some(color));
            }
            self.moves.truncate(self.cursor);
            self.moves.push((color, mv));
            self.cursor += 1;
        }
    }

    /// Removes the last move at the cursor.
    pub fn undo(&mut self) {
        if self.cursor > 0 {
            self.moves.truncate(self.cursor - 1);
            self.go_to(self.cursor - 1);
        }
    }

    /// Moves the cursor to `n` applied moves (clamped), rebuilding the board.
    pub fn go_to(&mut self, n: usize) {
        let n = n.min(self.moves.len());
        let mut board = Self::setup_board(&self.info);
        for (color, mv) in &self.moves[..n] {
            match mv {
                Move::Play { point } => {
                    // Records from servers are trusted; an illegal move is
                    // placed anyway so the replay never stops halfway.
                    if board.play(*point, *color).is_err() {
                        board.set(*point, Some(*color));
                    }
                }
                Move::Pass => board.pass(),
            }
        }
        self.board = board;
        self.cursor = n;
    }

    fn setup_board(info: &GameInfo) -> Board {
        let mut b = Board::new(info.size);
        for p in &info.setup_black {
            b.set(*p, Some(Color::Black));
        }
        for p in &info.setup_white {
            b.set(*p, Some(Color::White));
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternates_and_replays() {
        let mut g = Game::new(9, 6.5);
        g.play(Point::new(2, 2)).unwrap();
        g.play(Point::new(6, 6)).unwrap();
        g.pass();
        assert_eq!(g.to_move(), Color::White);
        g.go_to(1);
        assert_eq!(g.board().get(Point::new(6, 6)), None);
        assert_eq!(g.to_move(), Color::White);
        g.go_to(99);
        assert_eq!(g.cursor(), 3);
    }

    #[test]
    fn playing_mid_record_replaces_the_rest() {
        let mut g = Game::new(9, 6.5);
        g.play(Point::new(2, 2)).unwrap();
        g.play(Point::new(6, 6)).unwrap();
        g.go_to(1);
        g.play(Point::new(4, 4)).unwrap();
        assert_eq!(g.moves().len(), 2);
        assert_eq!(g.board().get(Point::new(6, 6)), None);
    }

    #[test]
    fn handicap_means_white_first() {
        let mut info = GameInfo { size: 19, komi: 0.5, ..Default::default() };
        info.setup_black = vec![Point::new(3, 3), Point::new(15, 15)];
        let g = Game::from_info(info);
        assert_eq!(g.to_move(), Color::White);
    }
}
