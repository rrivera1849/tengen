//! KataGo's JSON analysis engine, run as a child process.
//!
//! Each query is one line of JSON on KataGo's input; it answers with one line
//! per analysed turn on its output. Queries carry ids, so several can run at
//! once (a quick estimate while a review is analysing, say).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tengen_core::{Color, Game, Move, Point};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::AiError;

/// Column letters KataGo and GTP use: no I.
const LETTERS: &[u8] = b"ABCDEFGHJKLMNOPQRSTUVWXYZ";

/// `Point` (x from the left, y from the top) to KataGo's `Q16`.
pub fn to_gtp(p: Point, size: u8) -> String {
    format!("{}{}", LETTERS[p.x as usize] as char, size - p.y)
}

/// KataGo's `Q16` (or `pass`) back to a point; `None` for a pass or junk.
pub fn from_gtp(s: &str, size: u8) -> Option<Point> {
    let s = s.trim().to_ascii_uppercase();
    let mut chars = s.chars();
    let letter = chars.next()?;
    let x = LETTERS.iter().position(|&c| c as char == letter)? as u8;
    let row: u8 = chars.as_str().parse().ok()?;
    if x >= size || row == 0 || row > size {
        return None;
    }
    Some(Point::new(x, size - row))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    pub id: String,
    pub moves: Vec<[String; 2]>,
    pub initial_stones: Vec<[String; 2]>,
    pub rules: String,
    pub komi: f32,
    pub board_x_size: u8,
    pub board_y_size: u8,
    pub analyze_turns: Vec<usize>,
    pub max_visits: u32,
    pub include_ownership: bool,
}

impl Query {
    /// A query for `game`'s whole record; pick turns with `turns`.
    pub fn for_game(game: &Game, turns: Vec<usize>, max_visits: u32, include_ownership: bool) -> Query {
        let size = game.info.size;
        let c = |color: Color| if color == Color::Black { "B" } else { "W" }.to_string();
        let mut initial = Vec::new();
        for p in &game.info.setup_black {
            initial.push([c(Color::Black), to_gtp(*p, size)]);
        }
        for p in &game.info.setup_white {
            initial.push([c(Color::White), to_gtp(*p, size)]);
        }
        let moves = game
            .moves()
            .iter()
            .map(|(color, mv)| {
                let at = match mv {
                    Move::Play { point } => to_gtp(*point, size),
                    Move::Pass => "pass".into(),
                };
                [c(*color), at]
            })
            .collect();
        Query {
            id: String::new(),
            moves,
            initial_stones: initial,
            rules: rules_name(game.info.rules.as_deref()),
            komi: game.info.komi,
            board_x_size: size,
            board_y_size: size,
            analyze_turns: turns,
            max_visits,
            include_ownership,
        }
    }
}

/// Maps the rules named in a record to KataGo's names; Japanese if unknown.
fn rules_name(rules: Option<&str>) -> String {
    let r = rules.unwrap_or("").to_ascii_lowercase();
    match r.as_str() {
        "chinese" | "cn" => "chinese",
        "korean" => "korean",
        "aga" => "aga",
        "nz" | "new zealand" | "new-zealand" => "new-zealand",
        "tromp-taylor" | "tromp taylor" => "tromp-taylor",
        _ => "japanese",
    }
    .to_string()
}

/// KataGo's verdict on one position. Scores and win rates are from Black's
/// side (the config sets `reportAnalysisWinratesAs = BLACK`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnResult {
    pub turn_number: usize,
    pub root_info: RootInfo,
    #[serde(default)]
    pub move_infos: Vec<MoveInfo>,
    /// Row by row from the top left; +1 is Black's, -1 is White's.
    #[serde(default)]
    pub ownership: Option<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    pub winrate: f64,
    pub score_lead: f64,
    #[serde(default)]
    pub visits: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveInfo {
    #[serde(rename = "move")]
    pub mv: String,
    pub visits: u64,
    pub winrate: f64,
    pub score_lead: f64,
    #[serde(default)]
    pub pv: Vec<String>,
    pub order: u32,
}

impl TurnResult {
    /// KataGo's first choice.
    pub fn best(&self) -> Option<&MoveInfo> {
        self.move_infos.iter().min_by_key(|m| m.order)
    }

    pub fn ownership_at(&self, p: Point, size: u8) -> Option<f64> {
        self.ownership.as_ref()?.get(p.y as usize * size as usize + p.x as usize).copied()
    }
}

type Reply = Result<TurnResult, String>;

pub struct Engine {
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Reply>>>>,
    next_id: AtomicU64,
    _child: Child,
}

impl Engine {
    /// Starts `katago analysis` and waits until it is ready. The first start
    /// on a Mac can take a few minutes while KataGo prepares its network for
    /// the Neural Engine.
    pub async fn start(binary: &Path, model: &Path, config: &Path) -> Result<Engine, AiError> {
        let mut child = Command::new(binary)
            .arg("analysis")
            .arg("-config")
            .arg(config)
            .arg("-model")
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AiError::Start(e.to_string()))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        // KataGo reports start-up progress on stderr and says when it's ready.
        let (ready_tx, mut ready_rx) = mpsc::unbounded_channel::<Result<(), String>>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut tail: Vec<String> = Vec::new();
            let mut ready = false;
            while let Ok(Some(line)) = lines.next_line().await {
                if !ready && line.contains("ready to begin handling requests") {
                    ready = true;
                    let _ = ready_tx.send(Ok(()));
                }
                tail.push(line);
                if tail.len() > 12 {
                    tail.remove(0);
                }
            }
            if !ready {
                let _ = ready_tx.send(Err(tail.join("\n")));
            }
        });

        let pending: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Reply>>>> = Arc::default();
        let routes = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                route(&routes, &line);
            }
            // KataGo exited: fail everything still waiting.
            for (_, tx) in routes.lock().expect("routes").drain() {
                let _ = tx.send(Err("KataGo stopped".into()));
            }
        });

        match tokio::time::timeout(Duration::from_secs(900), ready_rx.recv()).await {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(log))) => return Err(AiError::Start(last_line(&log))),
            Ok(None) => return Err(AiError::Start("KataGo exited during start-up".into())),
            Err(_) => return Err(AiError::Start("KataGo took too long to start".into())),
        }
        Ok(Engine { stdin: tokio::sync::Mutex::new(stdin), pending, next_id: AtomicU64::new(1), _child: child })
    }

    /// Sends a query; results arrive on the returned channel, one per turn,
    /// in whatever order KataGo finishes them.
    pub async fn submit(&self, mut query: Query) -> Result<mpsc::UnboundedReceiver<Reply>, AiError> {
        query.id = format!("q{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::unbounded_channel();
        self.pending.lock().expect("pending").insert(query.id.clone(), tx);
        let mut line = serde_json::to_string(&query).map_err(|e| AiError::Other(e.to_string()))?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| AiError::Engine(e.to_string()))?;
        stdin.flush().await.map_err(|e| AiError::Engine(e.to_string()))?;
        Ok(rx)
    }

    /// Runs a query to completion. `progress(done, total)` is called as turns finish.
    pub async fn analyze(&self, query: Query, mut progress: impl FnMut(usize, usize)) -> Result<Vec<TurnResult>, AiError> {
        let total = query.analyze_turns.len();
        let mut rx = self.submit(query).await?;
        let mut out = Vec::with_capacity(total);
        while out.len() < total {
            match rx.recv().await {
                Some(Ok(r)) => {
                    out.push(r);
                    progress(out.len(), total);
                }
                Some(Err(e)) => return Err(AiError::Engine(e)),
                None => return Err(AiError::Engine("KataGo stopped answering".into())),
            }
        }
        out.sort_by_key(|r| r.turn_number);
        Ok(out)
    }
}

/// Hands one line of KataGo output to whoever is waiting for its id.
fn route(routes: &Mutex<HashMap<String, mpsc::UnboundedSender<Reply>>>, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let Some(id) = v.get("id").and_then(Value::as_str) else { return };
    let routes = routes.lock().expect("routes");
    let Some(tx) = routes.get(id) else { return };
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        let _ = tx.send(Err(err.to_string()));
        return;
    }
    if v.get("warning").is_some() || v.get("isDuringSearch").and_then(Value::as_bool) == Some(true) {
        return;
    }
    match serde_json::from_value::<TurnResult>(v) {
        Ok(r) => {
            let _ = tx.send(Ok(r));
        }
        Err(e) => {
            let _ = tx.send(Err(format!("unexpected KataGo output: {e}")));
        }
    }
}

fn last_line(log: &str) -> String {
    log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_coordinates() {
        assert_eq!(to_gtp(Point::new(15, 3), 19), "Q16");
        assert_eq!(to_gtp(Point::new(8, 18), 19), "J1", "column I is skipped");
        assert_eq!(from_gtp("q16", 19), Some(Point::new(15, 3)));
        assert_eq!(from_gtp("pass", 19), None);
        assert_eq!(from_gtp("T20", 19), None);
    }

    #[test]
    fn builds_a_query_from_a_record() {
        let mut g = Game::new(9, 5.5);
        g.play(Point::new(4, 4)).unwrap();
        g.pass();
        let q = Query::for_game(&g, vec![0, 1, 2], 100, false);
        assert_eq!(q.moves, vec![["B".to_string(), "E5".to_string()], ["W".to_string(), "pass".to_string()]]);
        let json = serde_json::to_string(&q).unwrap();
        assert!(json.contains(r#""boardXSize":9"#) && json.contains(r#""analyzeTurns":[0,1,2]"#));
        assert_eq!(rules_name(Some("nz")), "new-zealand");
    }

    #[test]
    fn routes_results_and_errors() {
        let routes: Mutex<HashMap<String, mpsc::UnboundedSender<Reply>>> = Mutex::default();
        let (tx, mut rx) = mpsc::unbounded_channel();
        routes.lock().unwrap().insert("q1".into(), tx);
        route(&routes, r#"{"id":"q1","isDuringSearch":false,"turnNumber":2,"rootInfo":{"winrate":0.6,"scoreLead":2.5,"visits":100},
            "moveInfos":[{"move":"D4","visits":50,"winrate":0.6,"scoreLead":2.5,"pv":["D4","Q16"],"order":0}]}"#);
        route(&routes, r#"{"id":"q1","error":"Illegal move 3: C3"}"#);
        route(&routes, r#"{"id":"other","turnNumber":0}"#);
        let first = rx.try_recv().unwrap().unwrap();
        assert_eq!((first.turn_number, first.best().unwrap().mv.as_str()), (2, "D4"));
        assert!(rx.try_recv().unwrap().is_err());
        assert!(rx.try_recv().is_err(), "nothing for other ids");
    }
}
