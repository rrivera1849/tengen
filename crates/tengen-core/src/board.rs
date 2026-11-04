use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    Black,
    White,
}

impl Color {
    pub fn opposite(self) -> Color {
        match self {
            Color::Black => Color::White,
            Color::White => Color::Black,
        }
    }
}

/// A board intersection. `x` runs left to right, `y` top to bottom, both from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: u8,
    pub y: u8,
}

impl Point {
    pub fn new(x: u8, y: u8) -> Self {
        Point { x, y }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
pub enum MoveError {
    #[error("point is off the board")]
    OffBoard,
    #[error("point is already occupied")]
    Occupied,
    #[error("move would recapture the ko immediately")]
    Ko,
    #[error("move is suicide")]
    Suicide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    size: u8,
    cells: Vec<Option<Color>>,
    /// The point that may not be played on the next move (simple ko).
    ko: Option<Point>,
    captures_black: u32,
    captures_white: u32,
}

impl Board {
    pub fn new(size: u8) -> Self {
        assert!((2..=25).contains(&size), "board size must be 2..=25");
        let n = size as usize * size as usize;
        Board { size, cells: vec![None; n], ko: None, captures_black: 0, captures_white: 0 }
    }

    pub fn size(&self) -> u8 {
        self.size
    }

    pub fn ko(&self) -> Option<Point> {
        self.ko
    }

    /// Stones captured *by* the given color so far.
    pub fn captures(&self, by: Color) -> u32 {
        match by {
            Color::Black => self.captures_black,
            Color::White => self.captures_white,
        }
    }

    pub fn get(&self, p: Point) -> Option<Color> {
        self.index(p).and_then(|i| self.cells[i])
    }

    /// Places a stone without rule checks or captures (used for SGF setup stones).
    pub fn set(&mut self, p: Point, c: Option<Color>) {
        if let Some(i) = self.index(p) {
            self.cells[i] = c;
        }
    }

    /// Plays a stone with full rules. Returns the captured stones.
    pub fn play(&mut self, p: Point, color: Color) -> Result<Vec<Point>, MoveError> {
        let i = self.index(p).ok_or(MoveError::OffBoard)?;
        if self.cells[i].is_some() {
            return Err(MoveError::Occupied);
        }
        if self.ko == Some(p) {
            return Err(MoveError::Ko);
        }

        self.cells[i] = Some(color);
        let mut captured = Vec::new();
        for n in self.neighbors(p) {
            if self.get(n) == Some(color.opposite()) {
                let group = self.group(n);
                if self.liberties(&group) == 0 {
                    for s in &group {
                        self.set(*s, None);
                    }
                    captured.extend(group);
                }
            }
        }

        let own = self.group(p);
        if captured.is_empty() && self.liberties(&own) == 0 {
            self.cells[i] = None;
            return Err(MoveError::Suicide);
        }

        // Simple ko: a single stone captured a single stone and now stands alone
        // with exactly one liberty, the point it just captured.
        self.ko = if captured.len() == 1 && own.len() == 1 && self.liberties(&own) == 1 {
            Some(captured[0])
        } else {
            None
        };

        match color {
            Color::Black => self.captures_black += captured.len() as u32,
            Color::White => self.captures_white += captured.len() as u32,
        }
        Ok(captured)
    }

    /// A pass clears any ko restriction.
    pub fn pass(&mut self) {
        self.ko = None;
    }

    pub fn group(&self, start: Point) -> Vec<Point> {
        let Some(color) = self.get(start) else { return Vec::new() };
        let mut seen = vec![false; self.cells.len()];
        let mut stack = vec![start];
        let mut out = Vec::new();
        while let Some(p) = stack.pop() {
            let i = self.index(p).expect("on board");
            if seen[i] || self.cells[i] != Some(color) {
                continue;
            }
            seen[i] = true;
            out.push(p);
            stack.extend(self.neighbors(p));
        }
        out
    }

    pub fn liberties(&self, group: &[Point]) -> usize {
        let mut libs: Vec<Point> = Vec::new();
        for s in group {
            for n in self.neighbors(*s) {
                if self.get(n).is_none() && !libs.contains(&n) {
                    libs.push(n);
                }
            }
        }
        libs.len()
    }

    /// Every stone on the board, row by row.
    pub fn stones(&self) -> impl Iterator<Item = (Point, Color)> + '_ {
        let size = self.size;
        self.cells.iter().enumerate().filter_map(move |(i, c)| {
            c.map(|c| (Point::new((i % size as usize) as u8, (i / size as usize) as u8), c))
        })
    }

    fn index(&self, p: Point) -> Option<usize> {
        (p.x < self.size && p.y < self.size)
            .then(|| p.y as usize * self.size as usize + p.x as usize)
    }

    fn neighbors(&self, p: Point) -> impl Iterator<Item = Point> {
        let s = self.size;
        let (x, y) = (p.x as i16, p.y as i16);
        [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
            .into_iter()
            .filter(move |&(a, b)| a >= 0 && b >= 0 && a < s as i16 && b < s as i16)
            .map(|(a, b)| Point::new(a as u8, b as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: u8, y: u8) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn captures_a_corner_stone() {
        let mut b = Board::new(9);
        b.play(p(0, 0), Color::White).unwrap();
        b.play(p(1, 0), Color::Black).unwrap();
        let cap = b.play(p(0, 1), Color::Black).unwrap();
        assert_eq!(cap, vec![p(0, 0)]);
        assert_eq!(b.get(p(0, 0)), None);
        assert_eq!(b.captures(Color::Black), 1);
    }

    #[test]
    fn rejects_suicide() {
        let mut b = Board::new(9);
        b.play(p(1, 0), Color::Black).unwrap();
        b.play(p(0, 1), Color::Black).unwrap();
        assert_eq!(b.play(p(0, 0), Color::White), Err(MoveError::Suicide));
        assert_eq!(b.get(p(0, 0)), None);
    }

    #[test]
    fn enforces_simple_ko() {
        // Classic ko shape around (1,1)/(2,1).
        let mut b = Board::new(9);
        for (x, y, c) in [
            (1, 0, Color::Black), (0, 1, Color::Black), (1, 2, Color::Black),
            (2, 0, Color::White), (3, 1, Color::White), (2, 2, Color::White),
            (2, 1, Color::Black),
        ] {
            b.play(p(x, y), c).unwrap();
        }
        // White captures the black stone at (2,1) by playing (1,1).
        assert_eq!(b.play(p(1, 1), Color::White).unwrap(), vec![p(2, 1)]);
        // Black may not retake immediately.
        assert_eq!(b.play(p(2, 1), Color::Black), Err(MoveError::Ko));
        // After a move elsewhere, the retake is legal.
        b.play(p(8, 8), Color::Black).unwrap();
        b.play(p(8, 0), Color::White).unwrap();
        assert_eq!(b.play(p(2, 1), Color::Black).unwrap(), vec![p(1, 1)]);
    }

    #[test]
    fn capturing_into_no_liberties_is_not_suicide() {
        let mut b = Board::new(9);
        b.play(p(0, 0), Color::White).unwrap();
        b.play(p(2, 0), Color::White).unwrap();
        b.play(p(1, 1), Color::White).unwrap();
        b.play(p(0, 1), Color::Black).unwrap();
        // Black at (1,0) has no liberties of its own but captures (0,0).
        assert_eq!(b.play(p(1, 0), Color::Black).unwrap(), vec![p(0, 0)]);
        assert_eq!(b.get(p(1, 0)), Some(Color::Black));
    }
}
