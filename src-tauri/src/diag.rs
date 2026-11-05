//! A short memory of what happened, for bug reports: connections, sign-ins,
//! games and messages shown, never passwords or tokens. "Copy diagnostics"
//! turns it into text with the player's names replaced.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many lines are kept; older ones fall off.
const KEEP: usize = 400;

static LOG: Mutex<Log> = Mutex::new(Log::new());

pub struct Log {
    lines: VecDeque<String>,
}

impl Log {
    pub const fn new() -> Self {
        Log {
            lines: VecDeque::new(),
        }
    }

    pub fn push(&mut self, line: String) {
        if self.lines.len() == KEEP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    /// Every line, oldest first, with each name in `names` replaced.
    pub fn text(&self, names: &[String]) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(&redact(line, names));
            out.push('\n');
        }
        out
    }
}

/// Notes one thing that happened, under a topic like "KGS" or "KataGo".
pub fn note(topic: &str, what: impl AsRef<str>) {
    let time = chrono::Local::now().format("%H:%M:%S");
    let line = format!("{time} [{topic}] {}", one_line(what.as_ref()));
    if let Ok(mut log) = LOG.lock() {
        log.push(line);
    }
}

/// The report: a header about this Mac and Tengen, then the log.
pub fn report(header: &[(&str, String)], names: &[String]) -> String {
    let mut out = String::from("Tengen diagnostics\n");
    for (k, v) in header {
        out.push_str(&format!("{k}: {}\n", redact(v, names)));
    }
    out.push('\n');
    if let Ok(log) = LOG.lock() {
        out.push_str(&log.text(names));
    }
    out
}

/// Replaces each name (ignoring case, whole words only) with "<you>".
pub fn redact(text: &str, names: &[String]) -> String {
    let mut out = text.to_string();
    for name in names.iter().map(|n| n.trim()).filter(|n| n.len() >= 2) {
        out = replace_word(&out, name, "<you>");
    }
    out
}

fn replace_word(text: &str, word: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let target = word.to_lowercase();
    // Lowercasing can change byte lengths outside ASCII; only then fall back
    // to a plain case-sensitive replace.
    if lower.len() != text.len() || target.len() != word.len() {
        return text.replace(word, with);
    }
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(&target) {
        let start = at + found;
        let end = start + target.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        out.push_str(&text[at..start]);
        if is_word(before) || is_word(after) {
            out.push_str(&text[start..end]);
        } else {
            out.push_str(with);
        }
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

fn one_line(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 300 {
        let cut: String = flat.chars().take(300).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_latest_lines() {
        let mut log = Log::new();
        for i in 0..KEEP + 5 {
            log.push(format!("line {i}"));
        }
        let text = log.text(&[]);
        assert_eq!(text.lines().count(), KEEP);
        assert!(text.starts_with("line 5\n"));
        assert!(text.ends_with(&format!("line {}\n", KEEP + 4)));
    }

    #[test]
    fn replaces_names_as_whole_words() {
        let names = vec!["Rafael".to_string(), "kenji99".to_string()];
        assert_eq!(
            redact("Signed in as rafael (12k); playing Kenji99", &names),
            "Signed in as <you> (12k); playing <you>"
        );
        assert_eq!(
            redact("Rafaelson is someone else", &names),
            "Rafaelson is someone else"
        );
        assert_eq!(redact("nothing here", &[]), "nothing here");
        assert_eq!(
            redact("a", &["".to_string(), "a".to_string()]),
            "a",
            "too-short names are left"
        );
    }

    #[test]
    fn notes_are_single_short_lines() {
        assert_eq!(one_line("two\nlines  here"), "two lines here");
        assert!(one_line(&"x".repeat(1000)).chars().count() <= 301);
    }

    #[test]
    fn report_has_a_header_and_redacts_it() {
        note("test", "connected as rafael");
        let r = report(
            &[
                ("Version", "1.0.0".into()),
                ("OGS", "rafael, connected".into()),
            ],
            &["rafael".into()],
        );
        assert!(r.starts_with("Tengen diagnostics\nVersion: 1.0.0\nOGS: <you>, connected\n"));
        assert!(r.contains("[test] connected as <you>"));
        assert!(!r.contains("rafael"));
    }
}
