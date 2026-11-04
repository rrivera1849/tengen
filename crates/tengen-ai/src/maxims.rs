//! The player's maxims across games, kept in a small local file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// The game's fingerprint, so one game counts once.
    pub game: String,
    /// The lesson's kind label, like "Life and death".
    pub kind: String,
    pub maxim: String,
}

#[derive(Debug, Default)]
pub struct Store {
    path: PathBuf,
    pub entries: Vec<Entry>,
}

impl Store {
    pub fn load(path: &Path) -> Store {
        let entries = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Store { path: path.to_path_buf(), entries }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.entries).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, json)
    }

    /// Replaces this game's maxims (a review run twice counts once).
    pub fn record(&mut self, game: &str, new: Vec<Entry>) {
        self.entries.retain(|e| e.game != game);
        self.entries.extend(new);
    }

    /// The most recent maxims from other games, newest first.
    pub fn recent(&self, except_game: &str, n: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in self.entries.iter().rev().filter(|e| e.game != except_game) {
            if !out.contains(&e.maxim) {
                out.push(e.maxim.clone());
            }
            if out.len() == n {
                break;
            }
        }
        out
    }

    /// "the third game in a row with a life and death lesson", when a kind
    /// appeared in each of the player's last two other reviewed games.
    pub fn recurring(&self, except_game: &str, kind: &str) -> Option<String> {
        let mut games: Vec<&str> = Vec::new();
        for e in self.entries.iter().rev().filter(|e| e.game != except_game) {
            if !games.contains(&e.game.as_str()) {
                games.push(&e.game);
            }
        }
        let streak = games
            .iter()
            .take_while(|g| self.entries.iter().any(|e| e.game == **g && e.kind == kind))
            .count();
        // `streak` earlier games plus this one: two earlier means the third.
        let ordinal = ["second", "third", "fourth", "fifth", "sixth"];
        (streak >= 2).then(|| {
            let nth = ordinal.get(streak - 1).copied().unwrap_or("latest");
            format!("the {nth} game in a row with a {} lesson", kind.to_lowercase())
        })
    }

    /// Every maxim, grouped by kind, most frequent kind first.
    pub fn by_kind(&self) -> Vec<(String, Vec<String>)> {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for e in self.entries.iter().rev() {
            match groups.iter_mut().find(|(k, _)| *k == e.kind) {
                Some((_, list)) if !list.contains(&e.maxim) => list.push(e.maxim.clone()),
                Some(_) => {}
                None => groups.push((e.kind.clone(), vec![e.maxim.clone()])),
            }
        }
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(game: &str, kind: &str, maxim: &str) -> Entry {
        Entry { date: "2026-11-04".into(), game: game.into(), kind: kind.into(), maxim: maxim.into() }
    }

    #[test]
    fn notices_a_streak_and_keeps_games_once() {
        let mut s = Store::default();
        s.record("g1", vec![e("g1", "Life and death", "Count eyes before attacking")]);
        s.record("g2", vec![e("g2", "Life and death", "Secure your base first"), e("g2", "Tempo", "Tenuki when it's small")]);
        assert_eq!(s.recurring("g3", "Life and death").as_deref(), Some("the third game in a row with a life and death lesson"));
        assert_eq!(s.recurring("g3", "Tempo"), None, "only one game");
        s.record("g2", vec![e("g2", "Tempo", "Tenuki when it's small")]);
        assert_eq!(s.entries.len(), 2, "re-reviewing replaces the game's maxims");
        assert_eq!(s.recent("g3", 5), vec!["Tenuki when it's small", "Count eyes before attacking"]);
    }

    #[test]
    fn saves_and_loads() {
        let path = std::env::temp_dir().join("tengen-maxims-test").join("maxims.json");
        let mut s = Store::load(&path);
        s.record("g", vec![e("g", "Endgame", "Play sente endgame first")]);
        s.save().unwrap();
        assert_eq!(Store::load(&path).entries, s.entries);
        let _ = std::fs::remove_file(&path);
    }
}
