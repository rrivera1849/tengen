//! Score estimates and AI reviews: KataGo on this Mac, lessons from Claude.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tengen_ai::analysis::{self, GameAnalysis};
use tengen_ai::estimate::{self, Estimate};
use tengen_ai::katago::Engine;
use tengen_ai::llm::{self, GameSummary, Lesson};
use tengen_ai::review::{self, Episode, Side};
use tengen_ai::{maxims, runtime, AiError};
use tengen_core::{Color, Game, Point};

use crate::{keychain, AppState};

#[derive(Default)]
pub struct AiState {
    engine: tokio::sync::Mutex<Option<Arc<Engine>>>,
    /// One review at a time.
    reviewing: tokio::sync::Mutex<()>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    katago: bool,
    api_key: bool,
}

/// Progress for the interface: `stage` is download, starting, quick, deep or lessons.
#[derive(Serialize, Clone)]
struct Progress {
    stage: &'static str,
    done: u64,
    total: u64,
}

fn progress(app: &AppHandle, stage: &'static str, done: u64, total: u64) {
    let _ = app.emit("ai-progress", Progress { stage, done, total });
}

fn data_dir(app: &AppHandle, sub: &str) -> PathBuf {
    let base = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("tengen"));
    base.join(sub)
}

pub fn status(app: &AppHandle) -> AiStatus {
    AiStatus {
        katago: runtime::locate(&data_dir(app, "katago")).is_some(),
        api_key: keychain::load_claude_key().is_some(),
    }
}

pub async fn install(app: &AppHandle) -> Result<AiStatus, String> {
    let dir = data_dir(app, "katago");
    let handle = app.clone();
    runtime::install(&dir, move |done, total| progress(&handle, "download", done, total))
        .await
        .map_err(|e| e.to_string())?;
    Ok(status(app))
}

/// Starts KataGo the first time it's needed and keeps it running.
async fn engine(app: &AppHandle) -> Result<Arc<Engine>, String> {
    let ai = app.state::<AiState>();
    let mut slot = ai.engine.lock().await;
    if let Some(e) = slot.as_ref() {
        return Ok(e.clone());
    }
    let rt = runtime::locate(&data_dir(app, "katago")).ok_or_else(|| AiError::NotInstalled.to_string())?;
    progress(app, "starting", 0, 1);
    let e = Arc::new(Engine::start(&rt.binary, &rt.model, &rt.config).await.map_err(|e| e.to_string())?);
    *slot = Some(e.clone());
    Ok(e)
}

/// Forgets a KataGo that stopped, so the next call starts a fresh one.
async fn reset_engine(app: &AppHandle) {
    *app.state::<AiState>().engine.lock().await = None;
}

fn current_game(app: &AppHandle) -> Game {
    app.state::<AppState>().game.lock().expect("game lock").clone()
}

pub async fn estimate(app: &AppHandle) -> Result<Estimate, String> {
    let game = current_game(app);
    let engine = engine(app).await?;
    match estimate::estimate(&engine, &game).await {
        Ok(e) => Ok(e),
        Err(err) => {
            reset_engine(app).await;
            Err(err.to_string())
        }
    }
}

// ---------- reviews ----------

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReviewView {
    fingerprint: String,
    you: Color,
    /// Black's score lead at every position, for the graph.
    leads: Vec<f64>,
    summary: Option<GameSummary>,
    lessons: Vec<LessonView>,
    /// Why there's no prose, when there isn't ("Add a Claude API key…").
    note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LessonView {
    side: Side,
    move_number: usize,
    color: Color,
    kind: String,
    points_lost: f64,
    played: Option<(u8, u8)>,
    best: Option<(u8, u8)>,
    line: Vec<(u8, u8)>,
    /// For your opponent's mistakes: your punishing line and your actual reply.
    answer_line: Vec<(u8, u8)>,
    your_reply: Option<(u8, u8)>,
    win_before: u32,
    win_after: u32,
    best_coord: String,
    played_coord: String,
    punish: Option<review::Punish>,
    lesson: Option<Lesson>,
    /// Set when the lesson failed its checks and only the facts are shown.
    problem: Option<String>,
}

/// Which side the player was, when Tengen can tell without asking.
pub fn guess_you(app: &AppHandle) -> Option<Color> {
    let state = app.state::<AppState>();
    let live = state.live.lock().expect("live lock");
    if let Some(c) = live.as_ref().and_then(|l| l.game.as_ref()).and_then(|g| g.my_color) {
        return Some(c);
    }
    let me = live.as_ref().map(|l| l.me.username.to_lowercase())?;
    drop(live);
    let game = state.game.lock().expect("game lock");
    match (&game.info.black, &game.info.white) {
        (Some(b), _) if b.to_lowercase() == me => Some(Color::Black),
        (_, Some(w)) if w.to_lowercase() == me => Some(Color::White),
        _ => None,
    }
}

pub async fn review(app: &AppHandle, you: Color) -> Result<ReviewView, String> {
    let ai = app.state::<AiState>();
    let _one = ai.reviewing.try_lock().map_err(|_| "A review is already running".to_string())?;
    let game = {
        let mut g = current_game(app);
        g.go_to(g.moves().len());
        g
    };
    if game.moves().len() < 10 {
        return Err("This game is too short to review".into());
    }
    let fingerprint = analysis::fingerprint(&game);
    let cache = data_dir(app, "reviews").join(format!("{fingerprint}-{}.json", color_key(you)));
    if let Some(cached) = std::fs::read_to_string(&cache).ok().and_then(|t| serde_json::from_str::<ReviewView>(&t).ok()) {
        let needs_prose = cached.note.is_some() && keychain::load_claude_key().is_some();
        if !needs_prose {
            attach_comments(app, &fingerprint, &cached);
            return Ok(cached);
        }
    }

    let analysis = analyse(app, &game).await.inspect_err(|_| {
        let h = app.clone();
        tauri::async_runtime::spawn(async move { reset_engine(&h).await });
    })?;
    let candidates = review::candidates(&game, &analysis.quick);
    let mut episodes = review::episodes(&game, &analysis, you, &candidates);

    // Context from earlier reviews.
    let store_path = data_dir(app, "maxims.json");
    let mut store = maxims::Store::load(&store_path);
    for e in &mut episodes {
        e.facts.earlier_maxims = store.recent(&fingerprint, 6);
        if e.side == Side::You {
            e.facts.recurring = store.recurring(&fingerprint, e.facts.kind);
        }
    }

    let key = keychain::load_claude_key();
    let mut written: Vec<(review::FactSheet, Lesson)> = Vec::new();
    let mut views: Vec<LessonView> = Vec::new();
    let total = episodes.len() as u64;
    for (n, e) in episodes.iter().enumerate() {
        progress(app, "lessons", n as u64, total);
        let (lesson, problem) = match &key {
            Some(k) => match llm::Client::new(k.clone()).write_lesson(&e.facts).await {
                Ok(l) => (Some(l), None),
                Err(AiError::Llm(p)) if p.contains("key was rejected") => return Err(p),
                Err(err) => (None, Some(err.to_string())),
            },
            None => (None, None),
        };
        if let Some(l) = &lesson {
            written.push((e.facts.clone(), l.clone()));
        }
        views.push(lesson_view(e, lesson, problem));
    }
    let summary = match (&key, written.is_empty()) {
        (Some(k), false) => llm::Client::new(k.clone()).write_summary(&written).await.ok(),
        _ => None,
    };

    // Keep the maxims for next time.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let entries = views
        .iter()
        .filter(|v| v.side == Side::You)
        .filter_map(|v| {
            Some(maxims::Entry { date: today.clone(), game: fingerprint.clone(), kind: v.kind.clone(), maxim: v.lesson.as_ref()?.maxim.clone() })
        })
        .collect::<Vec<_>>();
    if !entries.is_empty() {
        store.record(&fingerprint, entries);
        let _ = store.save();
    }

    let note = key.is_none().then(|| "Add a Claude API key to get written lessons and maxims.".to_string());
    let view = ReviewView { fingerprint: fingerprint.clone(), you, leads: analysis.leads(), summary, lessons: views, note };
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&cache, serde_json::to_string(&view).unwrap_or_default());
    }
    attach_comments(app, &fingerprint, &view);
    Ok(view)
}

/// Runs, or reloads, KataGo's two passes over the game.
async fn analyse(app: &AppHandle, game: &Game) -> Result<GameAnalysis, String> {
    let path = data_dir(app, "analysis").join(format!("{}.json", analysis::fingerprint(game)));
    if let Some(a) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<GameAnalysis>(&t).ok()) {
        return Ok(a);
    }
    let engine = engine(app).await?;
    let h = app.clone();
    let quick = analysis::quick_pass(&engine, game, move |d, t| progress(&h, "quick", d as u64, t as u64))
        .await
        .map_err(|e| e.to_string())?;
    let mut a = GameAnalysis { quick, ..Default::default() };
    let turns = review::deep_turns(&review::candidates(game, &a.quick));
    let h = app.clone();
    for r in analysis::deep_pass(&engine, game, turns, move |d, t| progress(&h, "deep", d as u64, t as u64))
        .await
        .map_err(|e| e.to_string())?
    {
        a.deep.insert(r.turn_number, r);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&path, serde_json::to_string(&a).unwrap_or_default());
    }
    Ok(a)
}

fn lesson_view(e: &Episode, lesson: Option<Lesson>, problem: Option<String>) -> LessonView {
    let xy = |p: Point| (p.x, p.y);
    LessonView {
        side: e.side,
        move_number: e.index + 1,
        color: e.color,
        kind: e.facts.kind.to_string(),
        points_lost: e.points_lost,
        played: e.played.map(xy),
        best: e.best.map(xy),
        line: e.line.iter().copied().map(xy).collect(),
        answer_line: e.answer_line.iter().copied().map(xy).collect(),
        your_reply: e.your_reply.map(xy),
        win_before: e.facts.win_chance_before,
        win_after: e.facts.win_chance_after,
        best_coord: e.facts.best.coord.clone(),
        played_coord: e.facts.played.coord.clone(),
        punish: e.facts.punish.clone(),
        lesson,
        problem,
    }
}

/// Writes each lesson onto its move, so saving the SGF keeps the review.
fn attach_comments(app: &AppHandle, fingerprint: &str, view: &ReviewView) {
    let state = app.state::<AppState>();
    let mut game = state.game.lock().expect("game lock");
    // Only onto the game that was reviewed.
    if analysis::fingerprint(&game) != fingerprint {
        return;
    }
    for l in &view.lessons {
        let Some(lesson) = &l.lesson else { continue };
        let who = if l.side == Side::You { "Your mistake" } else { "Opponent's mistake" };
        let text = format!(
            "Tengen review. {who}, {}: {}\n\n{}\n\nBetter: {}. {}\n\n{}\n\nMaxim: {}",
            l.kind, lesson.headline, lesson.what_happened, l.best_coord, lesson.better_idea, lesson.why, lesson.maxim
        );
        game.comments.insert(l.move_number, text);
    }
    if let Some(s) = &view.summary {
        game.comments.insert(0, format!("Tengen review. {}\n\n{}\n\nMaxim: {}", s.summary, s.theme, s.maxim));
    }
}

fn color_key(c: Color) -> &'static str {
    if c == Color::Black { "b" } else { "w" }
}

pub fn maxims(app: &AppHandle) -> Vec<(String, Vec<String>)> {
    maxims::Store::load(&data_dir(app, "maxims.json")).by_kind()
}
