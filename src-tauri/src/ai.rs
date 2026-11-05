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
    /// The running KataGo, and whether it has the human-style network loaded.
    engine: tokio::sync::Mutex<Option<(Arc<Engine>, bool)>>,
    /// One review at a time.
    reviewing: tokio::sync::Mutex<()>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    katago: bool,
    api_key: bool,
    /// KataGo's human-style network, for playing against KataGo.
    human_model: bool,
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

pub(crate) fn data_dir(app: &AppHandle, sub: &str) -> PathBuf {
    let base = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("tengen"));
    base.join(sub)
}

pub fn status(app: &AppHandle) -> AiStatus {
    let rt = runtime::locate(&data_dir(app, "katago"));
    AiStatus {
        katago: rt.is_some(),
        api_key: keychain::load_claude_key().is_some(),
        human_model: rt.is_some_and(|r| r.human_model.is_some()),
    }
}

/// Downloads KataGo's human-style network (about 100 MB), once.
pub async fn install_human(app: &AppHandle) -> Result<AiStatus, String> {
    let dir = data_dir(app, "katago");
    let handle = app.clone();
    crate::diag::note("KataGo", "downloading the human-style network");
    runtime::install_human_model(&dir, move |done, total| {
        progress(&handle, "download", done, total)
    })
    .await
    .map_err(|e| {
        crate::diag::note(
            "KataGo",
            format!("human-style network download failed: {e}"),
        );
        e.to_string()
    })?;
    Ok(status(app))
}

pub async fn install(app: &AppHandle) -> Result<AiStatus, String> {
    let dir = data_dir(app, "katago");
    let handle = app.clone();
    runtime::install(&dir, move |done, total| {
        progress(&handle, "download", done, total)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(status(app))
}

/// Starts KataGo the first time it's needed and keeps it running.
async fn engine(app: &AppHandle) -> Result<Arc<Engine>, String> {
    start_engine(app, false).await
}

/// KataGo with its human-style network loaded, restarting it if the network
/// arrived after KataGo started.
pub(crate) async fn engine_with_human(app: &AppHandle) -> Result<Arc<Engine>, String> {
    start_engine(app, true).await
}

async fn start_engine(app: &AppHandle, need_human: bool) -> Result<Arc<Engine>, String> {
    let ai = app.state::<AiState>();
    let mut slot = ai.engine.lock().await;
    if let Some((e, human)) = slot.as_ref() {
        if *human || !need_human {
            return Ok(e.clone());
        }
    }
    let rt = runtime::locate(&data_dir(app, "katago"))
        .ok_or_else(|| AiError::NotInstalled.to_string())?;
    if need_human && rt.human_model.is_none() {
        return Err("KataGo's human-style network isn't downloaded yet".into());
    }
    progress(app, "starting", 0, 1);
    crate::diag::note(
        "KataGo",
        format!(
            "starting (human-style network: {})",
            rt.human_model.is_some()
        ),
    );
    let e = Arc::new(
        Engine::start(&rt.binary, &rt.model, rt.human_model.as_deref(), &rt.config)
            .await
            .map_err(|e| {
                crate::diag::note("KataGo", format!("couldn't start: {e}"));
                e.to_string()
            })?,
    );
    *slot = Some((e.clone(), rt.human_model.is_some()));
    Ok(e)
}

/// Forgets a KataGo that stopped, so the next call starts a fresh one.
async fn reset_engine(app: &AppHandle) {
    *app.state::<AiState>().engine.lock().await = None;
}

fn current_game(app: &AppHandle) -> Game {
    app.state::<AppState>()
        .game
        .lock()
        .expect("game lock")
        .clone()
}

pub async fn estimate(app: &AppHandle) -> Result<Estimate, String> {
    estimate_game(app, &current_game(app)).await
}

/// The estimate for `game` at its cursor.
pub async fn estimate_game(app: &AppHandle, game: &Game) -> Result<Estimate, String> {
    let game = game.clone();
    let engine = engine(app).await?;
    match estimate::estimate(&engine, &game).await {
        Ok(e) => Ok(e),
        Err(err) => {
            reset_engine(app).await;
            Err(err.to_string())
        }
    }
}

/// Scores a move tried in the review's "Try again" at the position after
/// `turn` moves of the game on the board.
pub async fn judge(
    app: &AppHandle,
    turn: usize,
    tried: Point,
    played_loss: f64,
) -> Result<tengen_ai::judge::Judgement, String> {
    let game = current_game(app);
    if turn > game.moves().len() {
        return Err("That position isn't in this game".into());
    }
    let engine = engine(app).await?;
    tengen_ai::judge::judge(&engine, &game, turn, tried, played_loss)
        .await
        .map_err(|e| match e {
            AiError::Other(m) => m,
            other => {
                let h = app.clone();
                tauri::async_runtime::spawn(async move { reset_engine(&h).await });
                other.to_string()
            }
        })
}

/// The maxim to keep in mind for the next game: your most recent one.
pub fn next_maxim(app: &AppHandle) -> Option<String> {
    let store = maxims::Store::load(&data_dir(app, "maxims.json"));
    store.recent("", 1).into_iter().next()
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
    if let Some(b) = state.bot.lock().expect("bot lock").as_ref() {
        return Some(b.you);
    }
    {
        // A game against KataGo, reopened from the library: you're "You".
        let game = state.game.lock().expect("game lock");
        if game.info.server.as_deref() == Some("KataGo") {
            let you = |n: &Option<String>| n.as_deref() == Some("You");
            match (you(&game.info.black), you(&game.info.white)) {
                (true, false) => return Some(Color::Black),
                (false, true) => return Some(Color::White),
                _ => {}
            }
        }
    }
    let mut names = crate::sessions::usernames(&state);
    {
        let live = state.live.lock().expect("live lock");
        if let Some(c) = live
            .as_ref()
            .and_then(|l| l.game.as_ref())
            .and_then(|g| g.my_color)
        {
            return Some(c);
        }
        names.extend(live.as_ref().map(|l| l.me.username.to_lowercase()));
    }
    if let Some(c) = crate::sessions::my_color(&state) {
        return Some(c);
    }
    let game = state.game.lock().expect("game lock");
    let is_me = |n: &Option<String>| {
        n.as_ref()
            .is_some_and(|n| names.contains(&n.to_lowercase()))
    };
    match (is_me(&game.info.black), is_me(&game.info.white)) {
        (true, false) => Some(Color::Black),
        (false, true) => Some(Color::White),
        _ => None,
    }
}

pub async fn review(app: &AppHandle, you: Color) -> Result<ReviewView, String> {
    let ai = app.state::<AiState>();
    let _one = ai
        .reviewing
        .try_lock()
        .map_err(|_| "A review is already running".to_string())?;
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
    if let Some(cached) = std::fs::read_to_string(&cache)
        .ok()
        .and_then(|t| serde_json::from_str::<ReviewView>(&t).ok())
    {
        let needs_prose = cached.note.is_some() && keychain::load_claude_key().is_some();
        if !needs_prose {
            attach_comments(app, &fingerprint, &cached);
            // A saved review still counts toward your maxims, in case they
            // were never recorded (or the maxims file was lost since).
            remember_maxims(app, &cached, &today());
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
        (Some(k), false) => llm::Client::new(k.clone())
            .write_summary(&written)
            .await
            .ok(),
        _ => None,
    };

    // Keep the maxims for next time.
    let entries = maxim_entries(&fingerprint, &views, &today());
    if !entries.is_empty() {
        store.record(&fingerprint, entries);
        if let Err(e) = store.save() {
            crate::notify(app, format!("Couldn't save your maxims: {e}"));
        }
    }

    let note = key
        .is_none()
        .then(|| "Add a Claude API key to get written lessons and maxims.".to_string());
    let view = ReviewView {
        fingerprint: fingerprint.clone(),
        you,
        leads: analysis.leads(),
        summary,
        lessons: views,
        note,
    };
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
    if let Some(a) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<GameAnalysis>(&t).ok())
    {
        return Ok(a);
    }
    let engine = engine(app).await?;
    let h = app.clone();
    let quick = analysis::quick_pass(&engine, game, move |d, t| {
        progress(&h, "quick", d as u64, t as u64)
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut a = GameAnalysis {
        quick,
        ..Default::default()
    };
    let turns = review::deep_turns(&review::candidates(game, &a.quick));
    let h = app.clone();
    for r in analysis::deep_pass(&engine, game, turns, move |d, t| {
        progress(&h, "deep", d as u64, t as u64)
    })
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
        let who = if l.side == Side::You {
            "Your mistake"
        } else {
            "Opponent's mistake"
        };
        let text = format!(
            "Tengen review. {who}, {}: {}\n\n{}\n\nBetter: {}. {}\n\n{}\n\nMaxim: {}",
            l.kind,
            lesson.headline,
            lesson.what_happened,
            l.best_coord,
            lesson.better_idea,
            lesson.why,
            lesson.maxim
        );
        game.comments.insert(l.move_number, text);
    }
    if let Some(s) = &view.summary {
        game.comments.insert(
            0,
            format!(
                "Tengen review. {}\n\n{}\n\nMaxim: {}",
                s.summary, s.theme, s.maxim
            ),
        );
    }
}

fn color_key(c: Color) -> &'static str {
    if c == Color::Black {
        "b"
    } else {
        "w"
    }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Your maxims from a review's lessons (your opponent's are left out).
fn maxim_entries(game: &str, lessons: &[LessonView], date: &str) -> Vec<maxims::Entry> {
    lessons
        .iter()
        .filter(|v| v.side == Side::You)
        .filter_map(|v| {
            let maxim = v.lesson.as_ref()?.maxim.trim().to_string();
            (!maxim.is_empty()).then(|| maxims::Entry {
                date: date.to_string(),
                game: game.to_string(),
                kind: v.kind.clone(),
                maxim,
            })
        })
        .collect()
}

/// Records a review's maxims unless the store already has that game.
fn remember_maxims(app: &AppHandle, view: &ReviewView, date: &str) {
    let path = data_dir(app, "maxims.json");
    let mut store = maxims::Store::load(&path);
    if store.has_game(&view.fingerprint) {
        return;
    }
    let entries = maxim_entries(&view.fingerprint, &view.lessons, date);
    if !entries.is_empty() {
        store.record(&view.fingerprint, entries);
        let _ = store.save();
    }
}

/// Every maxim, grouped by kind. Reviews saved on disk whose maxims were
/// never recorded are picked up here, so none go missing.
pub fn maxims(app: &AppHandle) -> Result<Vec<(String, Vec<String>)>, String> {
    let path = data_dir(app, "maxims.json");
    let mut store = maxims::Store::load(&path);
    let mut changed = false;
    if let Ok(dir) = std::fs::read_dir(data_dir(app, "reviews")) {
        for entry in dir.flatten() {
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(view) = serde_json::from_str::<ReviewView>(&text) else {
                continue;
            };
            if store.has_game(&view.fingerprint) {
                continue;
            }
            let date = entry
                .metadata()
                .and_then(|m| m.modified())
                .map(|t| {
                    chrono::DateTime::<chrono::Local>::from(t)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_else(|_| today());
            let entries = maxim_entries(&view.fingerprint, &view.lessons, &date);
            if !entries.is_empty() {
                store.record(&view.fingerprint, entries);
                changed = true;
            }
        }
    }
    if changed {
        store
            .save()
            .map_err(|e| format!("Couldn't save your maxims: {e}"))?;
    }
    Ok(store.by_kind())
}
