//! Reading and writing SGF (Smart Game Format, FF[4]) game records.
//!
//! Reading follows the main line only: at every branch the first variation
//! is taken. That covers server downloads and almost every shared record.

use thiserror::Error;

use crate::board::{Color, Point};
use crate::game::{Game, GameInfo, Move};

#[derive(Debug, Error, PartialEq)]
pub enum SgfError {
    #[error("not an SGF file: no game tree found")]
    NoGameTree,
    #[error("unexpected end of file")]
    UnexpectedEof,
    #[error("unexpected character '{0}' at byte {1}")]
    Unexpected(char, usize),
    #[error("unsupported game type GM[{0}]; only Go (GM[1]) is supported")]
    NotGo(String),
    #[error("unsupported board size {0}")]
    BadSize(String),
    #[error("bad point '{0}'")]
    BadPoint(String),
}

type Node = Vec<(String, Vec<String>)>;

/// Parses the first game in an SGF collection.
pub fn parse(text: &str) -> Result<Game, SgfError> {
    let nodes = MainLine::new(text).read()?;
    let root = nodes.first().ok_or(SgfError::NoGameTree)?;

    let first = |node: &Node, key: &str| {
        node.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.first().cloned())
    };
    let text_prop = |key: &str| first(root, key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

    if let Some(gm) = first(root, "GM") {
        if gm.trim() != "1" {
            return Err(SgfError::NotGo(gm));
        }
    }
    let size = match first(root, "SZ") {
        None => 19,
        Some(s) => {
            let n: u8 = s.split(':').next().unwrap_or("").trim().parse().map_err(|_| SgfError::BadSize(s.clone()))?;
            if !(2..=25).contains(&n) {
                return Err(SgfError::BadSize(s));
            }
            n
        }
    };

    let mut info = GameInfo {
        size,
        komi: first(root, "KM").and_then(|k| k.trim().parse().ok()).unwrap_or(0.0),
        black: text_prop("PB"),
        white: text_prop("PW"),
        black_rank: text_prop("BR"),
        white_rank: text_prop("WR"),
        result: text_prop("RE"),
        date: text_prop("DT"),
        rules: text_prop("RU"),
        server: text_prop("PC"),
        event: text_prop("EV"),
        ..Default::default()
    };
    for (key, values) in root {
        match key.as_str() {
            "AB" => info.setup_black.extend(points(values, size)?),
            "AW" => info.setup_white.extend(points(values, size)?),
            _ => {}
        }
    }

    let mut game = Game::from_info(info);
    for node in &nodes {
        for (key, values) in node {
            let color = match key.as_str() {
                "B" => Color::Black,
                "W" => Color::White,
                _ => continue,
            };
            let mv = match point(values.first().map(String::as_str).unwrap_or(""), size)? {
                Some(p) => Move::Play { point: p },
                None => Move::Pass,
            };
            game.play_record(color, mv);
        }
    }
    Ok(game)
}

/// Writes the whole record (not just up to the cursor) as SGF.
pub fn write(game: &Game) -> String {
    let i = &game.info;
    let mut out = String::from("(;FF[4]GM[1]CA[UTF-8]");
    out.push_str(&format!("AP[Tengen:{}]", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("SZ[{}]KM[{}]", i.size, i.komi));
    for (key, value) in [
        ("PB", &i.black), ("BR", &i.black_rank), ("PW", &i.white), ("WR", &i.white_rank),
        ("RE", &i.result), ("DT", &i.date), ("RU", &i.rules), ("EV", &i.event), ("PC", &i.server),
    ] {
        if let Some(v) = value {
            out.push_str(&format!("{key}[{}]", escape(v)));
        }
    }
    for (key, stones) in [("AB", &i.setup_black), ("AW", &i.setup_white)] {
        if !stones.is_empty() {
            out.push_str(key);
            for p in stones {
                out.push_str(&format!("[{}]", coord(*p)));
            }
        }
    }
    for (n, (color, mv)) in game.moves().iter().enumerate() {
        if n % 10 == 0 {
            out.push('\n');
        }
        let key = if *color == Color::Black { "B" } else { "W" };
        let value = match mv {
            Move::Play { point } => coord(*point),
            Move::Pass => String::new(),
        };
        out.push_str(&format!(";{key}[{value}]"));
    }
    out.push_str(")\n");
    out
}

/// A filename that sorts by date and says who played where, safe on macOS:
/// `2026-11-04 21.34 OGS Rafael (3d) vs Opponent (2k).sgf`.
/// Missing parts are left out; a game from no server is labelled `Local`.
pub fn suggested_filename(game: &Game) -> String {
    let i = &game.info;
    let player = |name: &Option<String>, rank: &Option<String>, fallback: &str| {
        let name = name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(fallback);
        match rank.as_deref().filter(|r| !r.trim().is_empty()) {
            Some(r) => format!("{} ({})", name.trim(), r.trim()),
            None => name.trim().to_string(),
        }
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = &i.date {
        // DT may list several days ("2026-11-03,04"); the first is the start.
        parts.push(d.split(',').next().unwrap_or(d).trim().to_string());
    }
    if let Some(t) = &i.time {
        parts.push(t.trim().replace(':', "."));
    }
    parts.push(i.server.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Local".into()));
    parts.push(format!(
        "{} vs {}",
        player(&i.black, &i.black_rank, "Black"),
        player(&i.white, &i.white_rank, "White")
    ));
    let name = parts.join(" ");
    let clean: String = name
        .chars()
        .map(|c| if matches!(c, '/' | ':' | '\\') { '-' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    format!("{}.sgf", clean.trim())
}

fn coord(p: Point) -> String {
    let c = |n: u8| (b'a' + n) as char;
    format!("{}{}", c(p.x), c(p.y))
}

fn point(v: &str, size: u8) -> Result<Option<Point>, SgfError> {
    let v = v.trim();
    if v.is_empty() || (v == "tt" && size <= 19) {
        return Ok(None);
    }
    let b = v.as_bytes();
    let to_n = |c: u8| match c {
        b'a'..=b'z' => Some(c - b'a'),
        b'A'..=b'Z' => Some(c - b'A' + 26),
        _ => None,
    };
    match (b.len(), b.first().and_then(|&c| to_n(c)), b.get(1).and_then(|&c| to_n(c))) {
        (2, Some(x), Some(y)) if x < size && y < size => Ok(Some(Point::new(x, y))),
        _ => Err(SgfError::BadPoint(v.to_string())),
    }
}

/// Expands point lists, including compressed rectangles like `aa:cc`.
fn points(values: &[String], size: u8) -> Result<Vec<Point>, SgfError> {
    let mut out = Vec::new();
    for v in values {
        if let Some((a, b)) = v.split_once(':') {
            let (Some(a), Some(b)) = (point(a, size)?, point(b, size)?) else {
                return Err(SgfError::BadPoint(v.clone()));
            };
            for y in a.y.min(b.y)..=a.y.max(b.y) {
                for x in a.x.min(b.x)..=a.x.max(b.x) {
                    out.push(Point::new(x, y));
                }
            }
        } else if let Some(p) = point(v, size)? {
            out.push(p);
        }
    }
    Ok(out)
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace(']', "\\]")
}

/// Reads the nodes along the first variation of the first game tree.
struct MainLine<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> MainLine<'a> {
    fn new(text: &'a str) -> Self {
        MainLine { src: text.as_bytes(), pos: 0 }
    }

    fn read(mut self) -> Result<Vec<Node>, SgfError> {
        // Skip anything before the first game tree (BOM, whitespace, junk).
        while self.pos < self.src.len() && self.src[self.pos] != b'(' {
            self.pos += 1;
        }
        if self.pos >= self.src.len() {
            return Err(SgfError::NoGameTree);
        }
        let mut nodes = Vec::new();
        self.tree(&mut nodes, true)?;
        Ok(nodes)
    }

    /// Parses `( sequence subtrees* )`. Only the first subtree adds nodes.
    fn tree(&mut self, nodes: &mut Vec<Node>, keep: bool) -> Result<(), SgfError> {
        self.expect(b'(')?;
        let mut first_child = true;
        loop {
            self.skip_ws();
            match self.peek()? {
                b';' => {
                    self.pos += 1;
                    let node = self.node()?;
                    if keep {
                        nodes.push(node);
                    }
                }
                b'(' => {
                    self.tree(nodes, keep && first_child)?;
                    first_child = false;
                }
                b')' => {
                    self.pos += 1;
                    return Ok(());
                }
                c => return Err(SgfError::Unexpected(c as char, self.pos)),
            }
        }
    }

    fn node(&mut self) -> Result<Node, SgfError> {
        let mut props = Vec::new();
        loop {
            self.skip_ws();
            let c = self.peek()?;
            if !c.is_ascii_alphabetic() {
                return Ok(props);
            }
            let mut ident = String::new();
            while self.peek()?.is_ascii_alphabetic() {
                let c = self.src[self.pos];
                // FF[3] allowed lowercase letters inside identifiers; drop them.
                if c.is_ascii_uppercase() {
                    ident.push(c as char);
                }
                self.pos += 1;
            }
            let mut values = Vec::new();
            loop {
                self.skip_ws();
                if self.peek()? != b'[' {
                    break;
                }
                values.push(self.value()?);
            }
            props.push((ident, values));
        }
    }

    fn value(&mut self) -> Result<String, SgfError> {
        self.expect(b'[')?;
        let mut out = Vec::new();
        loop {
            let c = *self.src.get(self.pos).ok_or(SgfError::UnexpectedEof)?;
            self.pos += 1;
            match c {
                b'\\' => {
                    let next = *self.src.get(self.pos).ok_or(SgfError::UnexpectedEof)?;
                    self.pos += 1;
                    // A backslash before a newline is a soft line break.
                    if next != b'\n' && next != b'\r' {
                        out.push(next);
                    }
                }
                b']' => break,
                _ => out.push(c),
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Result<u8, SgfError> {
        self.src.get(self.pos).copied().ok_or(SgfError::UnexpectedEof)
    }

    fn expect(&mut self, c: u8) -> Result<(), SgfError> {
        self.skip_ws();
        match self.peek()? {
            x if x == c => {
                self.pos += 1;
                Ok(())
            }
            x => Err(SgfError::Unexpected(x as char, self.pos)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "(;FF[4]GM[1]SZ[19]KM[6.5]PB[Shin Jinseo]PW[Ke Jie]BR[9p]WR[9p]RE[B+R]DT[2026-05-01]
        ;B[pd];W[dp](;B[pp];W[dd])(;B[dd]))";

    #[test]
    fn parses_main_line_and_metadata() {
        let g = parse(SAMPLE).unwrap();
        assert_eq!(g.info.size, 19);
        assert_eq!(g.info.komi, 6.5);
        assert_eq!(g.info.black.as_deref(), Some("Shin Jinseo"));
        assert_eq!(g.info.result.as_deref(), Some("B+R"));
        assert_eq!(g.moves().len(), 4, "follows the first variation only");
        assert_eq!(g.moves()[0], (Color::Black, Move::Play { point: Point::new(15, 3) }));
    }

    #[test]
    fn round_trips() {
        let g = parse(SAMPLE).unwrap();
        let again = parse(&write(&g)).unwrap();
        assert_eq!(again.info, g.info);
        assert_eq!(again.moves(), g.moves());
    }

    #[test]
    fn handles_passes_handicap_and_escapes() {
        let g = parse(r"(;SZ[9]AB[cc:dc]PW[a \] b];W[];B[tt])").unwrap();
        assert_eq!(g.info.setup_black, vec![Point::new(2, 2), Point::new(3, 2)]);
        assert_eq!(g.info.white.as_deref(), Some("a ] b"));
        assert_eq!(g.moves(), &[(Color::White, Move::Pass), (Color::Black, Move::Pass)]);
        assert!(write(&g).contains(r"PW[a \] b]"));
    }

    #[test]
    fn rejects_other_games() {
        assert_eq!(parse("(;GM[3];B[aa])").unwrap_err(), SgfError::NotGo("3".into()));
        assert_eq!(parse("hello").unwrap_err(), SgfError::NoGameTree);
    }

    #[test]
    fn filename_names_server_players_ranks_and_time() {
        let mut g = parse("(;PB[Rafael]BR[3d]PW[Opp]WR[2k]DT[2026-11-04]PC[OGS])").unwrap();
        g.info.time = Some("21:34".into());
        assert_eq!(suggested_filename(&g), "2026-11-04 21.34 OGS Rafael (3d) vs Opp (2k).sgf");
    }

    #[test]
    fn filename_without_details_is_still_useful_and_safe() {
        let g = parse("(;PB[a/b]PW[c]DT[2026-11-03,04])").unwrap();
        assert_eq!(suggested_filename(&g), "2026-11-03 Local a-b vs c.sgf");
        assert_eq!(suggested_filename(&Game::new(19, 6.5)), "Local Black vs White.sgf");
    }
}
