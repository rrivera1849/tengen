//! Turning fact sheets into teaching, and checking what comes back.
//!
//! The LLM never judges a position. It receives a fact sheet computed from
//! KataGo and Tengen's board, and is held to it: every coordinate and number
//! it writes must appear in the sheet. A failed check gets one retry; if that
//! fails too, the app shows the facts without prose.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::review::{is_coordinate, FactSheet};
use crate::AiError;

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2026-10-21";
pub const MODEL: &str = "claude-sonnet-5-5";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub headline: String,
    pub what_happened: String,
    pub better_idea: String,
    pub why: String,
    pub maxim: String,
    /// The coordinates the lesson talks about.
    #[serde(default)]
    pub points: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSummary {
    pub summary: String,
    pub theme: String,
    pub maxim: String,
}

const SYSTEM: &str = "You are a patient Go teacher writing a short lesson for one player about one moment in their game.

You receive a fact sheet computed by the KataGo engine and checked by the app. It is the only source of truth. You cannot see the board except through it, and you must not work out Go facts yourself: no reading of variations, no claims about life and death, liberties, territory or score that the sheet does not state. If the sheet doesn't support something, leave it out.

How to write:
- Speak to the player directly (\"you\"). `side` says whether the mistake was theirs or their opponent's. For an opponent's mistake, explain what the opponent got wrong and how the player could have punished it (`punish`), and say plainly whether they found the answer.
- Pitch it at `your_rank`. For kyu players, gloss Go terms in a few words the first time (\"tenuki, playing elsewhere\").
- Explain ideas, not long sequences. Mention at most three coordinates, written exactly as in the sheet (like Q16).
- Only quote numbers that appear in the sheet: points_lost, the win chances, or territory_swing points.
- headline: at most 12 words, naming the idea, not the move.
- what_happened and better_idea: two or three sentences each. why: two to four sentences on the reasoning.
- maxim: one sentence of at most 14 words, general enough to reuse in other games and concrete enough to act on. No coordinates. \"Play good shape\" is too vague; \"Answer a peep before it becomes a cut\" is right.
- If `recurring` is set, mention the pattern once in why. If `earlier_maxims` holds one that fits, you may refer back to it instead of inventing a near-duplicate.

Write the lesson with the write_lesson tool.";

const SUMMARY_SYSTEM: &str = "You are a patient Go teacher. You receive the lessons already written about one game, each with its fact sheet. Write a two-sentence summary of how the game went for the player, the common thread across their own mistakes (theme), and one maxim of at most 14 words for their next game. Use only what the lessons and sheets state; no coordinates in the maxim. Use the write_summary tool.";

fn lesson_tool() -> Value {
    json!({
        "name": "write_lesson",
        "description": "Write the lesson for this moment of the game.",
        "input_schema": {
            "type": "object",
            "properties": {
                "headline": {"type": "string"},
                "what_happened": {"type": "string"},
                "better_idea": {"type": "string"},
                "why": {"type": "string"},
                "maxim": {"type": "string"},
                "points": {"type": "array", "items": {"type": "string"}, "description": "Coordinates the lesson mentions, like Q16"}
            },
            "required": ["headline", "what_happened", "better_idea", "why", "maxim", "points"]
        }
    })
}

fn summary_tool() -> Value {
    json!({
        "name": "write_summary",
        "description": "Write the summary of the game's lessons.",
        "input_schema": {
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "theme": {"type": "string"},
                "maxim": {"type": "string"}
            },
            "required": ["summary", "theme", "maxim"]
        }
    })
}

pub struct Client {
    http: reqwest::Client,
    key: String,
    model: String,
}

impl Client {
    pub fn new(key: impl Into<String>) -> Client {
        Client { http: reqwest::Client::new(), key: key.into(), model: MODEL.to_string() }
    }

    /// Writes the lesson for one episode. `Err(AiError::Llm)` with the
    /// problems listed when it fails its checks twice.
    pub async fn write_lesson(&self, sheet: &FactSheet) -> Result<Lesson, AiError> {
        let facts = serde_json::to_string_pretty(sheet).map_err(|e| AiError::Other(e.to_string()))?;
        let mut prompt = format!("Fact sheet:\n{facts}");
        for attempt in 0..2 {
            let input = self.call(SYSTEM, &prompt, lesson_tool(), "write_lesson").await?;
            let lesson: Lesson = serde_json::from_value(input).map_err(|e| AiError::Llm(format!("malformed lesson: {e}")))?;
            let problems = check(&lesson, sheet);
            if problems.is_empty() {
                return Ok(lesson);
            }
            if attempt == 1 {
                return Err(AiError::Llm(problems.join("; ")));
            }
            prompt = format!(
                "Fact sheet:\n{facts}\n\nA draft of this lesson broke the rules:\n- {}\nWrite it again, using only what the fact sheet states.",
                problems.join("\n- ")
            );
        }
        unreachable!("two attempts")
    }

    /// The game-level summary, theme and maxim.
    pub async fn write_summary(&self, lessons: &[(FactSheet, Lesson)]) -> Result<GameSummary, AiError> {
        let body: Vec<Value> = lessons
            .iter()
            .map(|(sheet, lesson)| json!({"facts": sheet, "lesson": lesson}))
            .collect();
        let prompt = serde_json::to_string_pretty(&body).map_err(|e| AiError::Other(e.to_string()))?;
        let input = self.call(SUMMARY_SYSTEM, &prompt, summary_tool(), "write_summary").await?;
        let s: GameSummary = serde_json::from_value(input).map_err(|e| AiError::Llm(format!("malformed summary: {e}")))?;
        if contains_coordinate(&s.maxim) {
            return Err(AiError::Llm("the game maxim named a coordinate".into()));
        }
        Ok(s)
    }

    /// One Messages API call that must answer with the named tool.
    async fn call(&self, system: &str, prompt: &str, tool: Value, name: &str) -> Result<Value, AiError> {
        let body = json!({
            "model": self.model,
            "max_tokens": 1500,
            "system": system,
            "messages": [{"role": "user", "content": prompt}],
            "tools": [tool],
            "tool_choice": {"type": "tool", "name": name, "disable_parallel_tool_use": true},
        });
        let resp = self
            .http
            .post(API_URL)
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Llm(format!("couldn't reach the Claude API: {e}")))?;
        let status = resp.status();
        let reply: Value = resp.json().await.map_err(|e| AiError::Llm(e.to_string()))?;
        if !status.is_success() {
            let message = reply.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
            return Err(AiError::Llm(match status.as_u16() {
                401 => "the Claude API key was rejected".into(),
                429 => "the Claude API is busy or the key hit its limit; try again shortly".into(),
                _ => message.to_string(),
            }));
        }
        tool_input(&reply, name).ok_or_else(|| AiError::Llm("the reply had no lesson in it".into()))
    }
}

fn tool_input(reply: &Value, name: &str) -> Option<Value> {
    reply
        .get("content")?
        .as_array()?
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use") && b.get("name").and_then(Value::as_str) == Some(name))?
        .get("input")
        .cloned()
}

/// What's wrong with a lesson, judged against its fact sheet. Empty means fine.
pub fn check(lesson: &Lesson, sheet: &FactSheet) -> Vec<String> {
    let mut problems = Vec::new();
    let allowed = sheet.coordinates();
    let text = [&lesson.headline, &lesson.what_happened, &lesson.better_idea, &lesson.why, &lesson.maxim];
    let mut named: Vec<String> = lesson.points.clone();
    for t in text {
        named.extend(words(t).filter(|w| is_coordinate(w)).map(str::to_string));
    }
    named.sort();
    named.dedup();
    for c in &named {
        if !allowed.contains(c) {
            problems.push(format!("{c} is not in the fact sheet"));
        }
    }
    if contains_coordinate(&lesson.maxim) {
        problems.push("the maxim names a coordinate".into());
    }
    if lesson.maxim.split_whitespace().count() > 14 {
        problems.push("the maxim is longer than 14 words".into());
    }
    if lesson.headline.split_whitespace().count() > 12 {
        problems.push("the headline is longer than 12 words".into());
    }
    let known = sheet.numbers();
    for t in text {
        for (n, unit) in quoted_numbers(t) {
            let ok = known.iter().any(|k| (k - n).abs() <= if unit == "%" { 2.0 } else { 0.6 });
            if !ok {
                problems.push(format!("{n}{} isn't a number from the fact sheet", if unit == "%" { "%" } else { " points" }));
            }
        }
    }
    problems
}

fn words(t: &str) -> impl Iterator<Item = &str> {
    t.split(|c: char| !c.is_ascii_alphanumeric())
}

fn contains_coordinate(t: &str) -> bool {
    words(t).any(is_coordinate)
}

/// Numbers written as points ("4.5 points") or percentages ("62%").
fn quoted_numbers(t: &str) -> Vec<(f64, &'static str)> {
    let tokens: Vec<&str> = t.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        let clean = tok.trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '%');
        if let Some(num) = clean.strip_suffix('%') {
            if let Ok(n) = num.parse::<f64>() {
                out.push((n, "%"));
            }
            continue;
        }
        let Ok(n) = clean.trim_end_matches('.').parse::<f64>() else { continue };
        let next = tokens.get(i + 1).map(|w| w.to_ascii_lowercase()).unwrap_or_default();
        if next.starts_with("point") {
            out.push((n, "points"));
        } else if next.starts_with("percent") {
            out.push((n, "%"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{Side, Spot, Swing};

    fn sheet() -> FactSheet {
        FactSheet {
            side: Side::You,
            move_number: 42,
            mover: "Black",
            kind: "Direction of play",
            phase: "middle game",
            mover_rank: Some("3k".into()),
            your_rank: Some("3k".into()),
            board: "   A B\n 2 . .\n 1 . .\n".into(),
            recent_moves: vec!["41 White R14".into()],
            played: Spot { coord: "C3".into(), area: "bottom left corner" },
            best: Spot { coord: "Q10".into(), area: "right side" },
            better_line: vec!["Black Q10".into(), "White R12".into()],
            points_lost: 6.5,
            win_chance_before: 55,
            win_chance_after: 38,
            group_changes: vec![],
            territory_swing: vec![Swing { area: "right side", points: 7.2 }],
            groups_under_pressure: vec![],
            punish: None,
            earlier_maxims: vec![],
            recurring: None,
        }
    }

    fn lesson() -> Lesson {
        Lesson {
            headline: "The right side mattered more than the corner".into(),
            what_happened: "You took a small point at C3 while White approached at R14.".into(),
            better_idea: "Q10 settles your stones on the right and reduces White's framework.".into(),
            why: "The swing on the right side was about 7.2 points, so this cost you 6.5 points.".into(),
            maxim: "When your opponent builds, choose the biggest area first".into(),
            points: vec!["C3".into(), "Q10".into(), "R14".into()],
        }
    }

    #[test]
    fn a_faithful_lesson_passes() {
        assert_eq!(check(&lesson(), &sheet()), Vec::<String>::new());
    }

    #[test]
    fn invented_coordinates_and_numbers_are_caught() {
        let mut l = lesson();
        l.better_idea = "D4 was better, worth 12 points, a 30% swing.".into();
        let problems = check(&l, &sheet());
        assert!(problems.iter().any(|p| p.starts_with("D4")));
        assert!(problems.iter().any(|p| p.starts_with("12 points") || p.contains("12 points")));
        assert!(problems.iter().any(|p| p.contains("30%")));
    }

    #[test]
    fn maxims_stay_general_and_short() {
        let mut l = lesson();
        l.maxim = "Always play Q10 when the opponent approaches from above in this exact kind of opening position".into();
        let problems = check(&l, &sheet());
        assert!(problems.iter().any(|p| p.contains("names a coordinate")));
        assert!(problems.iter().any(|p| p.contains("longer than 14")));
    }

    #[test]
    fn reads_the_tool_answer() {
        let reply = json!({"content": [{"type": "text", "text": "hi"},
            {"type": "tool_use", "id": "t", "name": "write_lesson", "input": {"headline": "h"}}]});
        assert_eq!(tool_input(&reply, "write_lesson").unwrap()["headline"], "h");
        assert!(tool_input(&reply, "other").is_none());
    }

    #[test]
    fn finds_quoted_numbers() {
        assert_eq!(quoted_numbers("lost 4.5 points, from 62% to 40 percent."), vec![(4.5, "points"), (62.0, "%"), (40.0, "%")]);
        assert!(quoted_numbers("move 42 at Q10").is_empty());
    }
}
