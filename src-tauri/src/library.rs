//! Your games: online and KataGo games save themselves here when they end,
//! and "Recent games" lists them, newest first, with whether you've reviewed
//! each one. Files are plain SGF in Tengen's Application Support folder.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tengen_ai::analysis::fingerprint;
use tengen_core::{sgf, Game};

/// How many games "Recent games" shows.
pub const SHOWN: usize = 200;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The file's name in the library folder; how the interface asks for it.
    pub file: String,
    pub date: Option<String>,
    pub black: String,
    pub white: String,
    pub black_rank: Option<String>,
    pub white_rank: Option<String>,
    pub result: Option<String>,
    pub server: Option<String>,
    pub size: u8,
    pub moves: usize,
    pub reviewed: bool,
}

/// Saves `game` into `dir` unless an identical record is already there.
/// Returns the new file, or None when it was already saved.
pub fn save_to(dir: &Path, game: &Game) -> std::io::Result<Option<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut record = game.clone();
    record.go_to(record.moves().len());
    let print = fingerprint(&record);
    for (_, g) in read_all(dir) {
        if fingerprint(&g) == print {
            return Ok(None);
        }
    }
    let name = sgf::suggested_filename(&record);
    let stem = name.trim_end_matches(".sgf").to_string();
    let mut path = dir.join(format!("{stem}.sgf"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).sgf"));
        n += 1;
    }
    std::fs::write(&path, sgf::write(&record))?;
    Ok(Some(path))
}

/// The library, newest first. A game counts as reviewed when a saved review
/// for it is in `reviews` (named by the game's fingerprint).
pub fn list_in(dir: &Path, reviews: &Path) -> Vec<Entry> {
    let mut all: Vec<(std::time::SystemTime, Entry)> = read_all(dir)
        .into_iter()
        .map(|(path, g)| {
            let modified = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let print = fingerprint(&g);
            let reviewed = ["b", "w"]
                .iter()
                .any(|c| reviews.join(format!("{print}-{c}.json")).exists());
            let i = &g.info;
            let entry = Entry {
                file: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                date: i.date.clone(),
                black: i.black.clone().unwrap_or_else(|| "Black".into()),
                white: i.white.clone().unwrap_or_else(|| "White".into()),
                black_rank: i.black_rank.clone(),
                white_rank: i.white_rank.clone(),
                result: i.result.clone(),
                server: i.server.clone(),
                size: i.size,
                moves: g.moves().len(),
                reviewed,
            };
            (modified, entry)
        })
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.file.cmp(&a.1.file)));
    all.into_iter().take(SHOWN).map(|(_, e)| e).collect()
}

/// A library file by name; None for anything that isn't a plain file name
/// of an `.sgf` in `dir`.
pub fn resolve(dir: &Path, file: &str) -> Option<PathBuf> {
    let plain = !file.is_empty()
        && !file.contains('/')
        && !file.contains('\\')
        && !file.starts_with('.')
        && file.ends_with(".sgf");
    let path = dir.join(file);
    (plain && path.is_file()).then_some(path)
}

pub fn load(path: &Path) -> Result<Game, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut game = sgf::parse(&text).map_err(|e| e.to_string())?;
    game.go_to(0);
    Ok(game)
}

fn read_all(dir: &Path) -> Vec<(PathBuf, Game)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sgf"))
        .filter_map(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            let g = sgf::parse(&text).ok()?;
            Some((p, g))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tengen_core::Point;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tengen-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn game(black: &str, moves: &[(u8, u8)]) -> Game {
        let mut g = Game::new(9, 7.0);
        g.info.black = Some(black.into());
        g.info.white = Some("KataGo".into());
        g.info.white_rank = Some("5k".into());
        g.info.date = Some("2026-11-04".into());
        g.info.time = Some("21:30".into());
        g.info.server = Some("KataGo".into());
        g.info.result = Some("B+3.5".into());
        for &(x, y) in moves {
            g.play(Point::new(x, y)).unwrap();
        }
        g
    }

    #[test]
    fn saves_once_and_lists_newest_first() {
        let dir = temp("save");
        let reviews = dir.join("reviews");
        let a = game("You", &[(2, 2), (6, 6)]);
        let first = save_to(&dir, &a).unwrap().expect("saved");
        assert!(first
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".sgf"));
        assert_eq!(
            save_to(&dir, &a).unwrap(),
            None,
            "the same game isn't saved twice"
        );

        // Same names and time, different moves: a second file beside it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = game("You", &[(2, 2), (6, 6), (4, 4)]);
        let second = save_to(&dir, &b).unwrap().expect("saved");
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with(" (2).sgf"));

        let list = list_in(&dir, &reviews);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].moves, 3, "newest first");
        assert_eq!(list[0].result.as_deref(), Some("B+3.5"));
        assert!(!list[0].reviewed);

        // A saved review for the first game marks it reviewed.
        std::fs::create_dir_all(&reviews).unwrap();
        let mut reloaded = load(&first).unwrap();
        reloaded.go_to(reloaded.moves().len());
        std::fs::write(
            reviews.join(format!("{}-b.json", fingerprint(&reloaded))),
            "{}",
        )
        .unwrap();
        let list = list_in(&dir, &reviews);
        assert!(list.iter().find(|e| e.moves == 2).unwrap().reviewed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_game_keeps_its_fingerprint() {
        // Reviews are cached by fingerprint, so a game reopened from the
        // library must match the one reviewed straight after playing it.
        let dir = temp("print");
        let mut g = game("You", &[(2, 2), (6, 6), (4, 4)]);
        g.comments.insert(1, "a comment".into());
        let path = save_to(&dir, &g).unwrap().unwrap();
        let mut back = load(&path).unwrap();
        back.go_to(back.moves().len());
        assert_eq!(fingerprint(&back), fingerprint(&g));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_only_plain_names_in_the_folder() {
        let dir = temp("resolve");
        let path = save_to(&dir, &game("You", &[(2, 2)])).unwrap().unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(resolve(&dir, &name), Some(path));
        assert_eq!(resolve(&dir, "../secret.sgf"), None);
        assert_eq!(resolve(&dir, "/etc/passwd"), None);
        assert_eq!(resolve(&dir, ".hidden.sgf"), None);
        assert_eq!(resolve(&dir, "missing.sgf"), None);
        assert_eq!(resolve(&dir, ""), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_files_that_arent_games() {
        let dir = temp("junk");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "hello").unwrap();
        std::fs::write(dir.join("broken.sgf"), "not an sgf").unwrap();
        assert!(list_in(&dir, &dir.join("reviews")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
