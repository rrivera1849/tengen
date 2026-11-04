//! The Tauri layer: commands the interface calls, plus the live OGS
//! connection. Game logic lives in the crates; this file only wires them up.

mod keychain;
mod live;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tengen_core::adapter::{ServerAdapter, Speed};
use tengen_core::{sgf, Color, Game, Move, Point};
use tengen_ogs::realtime::local_date_time_from_rfc3339;
use tengen_ogs::{parse_game_ref, OgsAdapter};

use live::Live;

pub struct AppState {
    pub game: Mutex<Game>,
    pub ogs: Mutex<OgsAdapter>,
    pub live: Mutex<Option<Live>>,
}

impl AppState {
    fn ogs(&self) -> OgsAdapter {
        self.ogs.lock().expect("ogs lock").clone()
    }
}

/// Everything the interface needs to draw the board and its edges.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct View {
    size: u8,
    stones: Vec<(u8, u8, Color)>,
    to_move: Color,
    cursor: usize,
    total: usize,
    last: Option<(u8, u8)>,
    last_was_pass: bool,
    captures_black: u32,
    captures_white: u32,
    black: String,
    white: String,
    black_rank: Option<String>,
    white_rank: Option<String>,
    result: Option<String>,
    server: Option<String>,
    komi: f32,
    /// Set while a live game is open.
    live: Option<live::LiveView>,
}

pub fn view(game: &Game, live: Option<&Live>) -> View {
    let b = game.board();
    let last = game.last_move();
    View {
        size: b.size(),
        stones: b.stones().map(|(p, c)| (p.x, p.y, c)).collect(),
        to_move: game.to_move(),
        cursor: game.cursor(),
        total: game.moves().len(),
        last: match last {
            Some((_, Move::Play { point })) => Some((point.x, point.y)),
            _ => None,
        },
        last_was_pass: matches!(last, Some((_, Move::Pass))),
        captures_black: b.captures(Color::Black),
        captures_white: b.captures(Color::White),
        black: game.info.black.clone().unwrap_or_else(|| "Black".into()),
        white: game.info.white.clone().unwrap_or_else(|| "White".into()),
        black_rank: game.info.black_rank.clone(),
        white_rank: game.info.white_rank.clone(),
        result: game.info.result.clone(),
        server: game.info.server.clone(),
        komi: game.info.komi,
        live: live.and_then(Live::view),
    }
}

/// The current view, read under the locks.
pub fn current_view(state: &AppState) -> View {
    let live = state.live.lock().expect("live lock");
    let game = state.game.lock().expect("game lock");
    view(&game, live.as_ref())
}

pub fn push_view(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = app.emit("view", current_view(&state));
}

pub fn notify(app: &AppHandle, text: impl Into<String>) {
    let _ = app.emit("notice", text.into());
}

/// The live game id, when one is open and the given command should go to OGS.
fn live_game(state: &AppState) -> Option<(tengen_ogs::realtime::Realtime, live::LiveGame)> {
    let live = state.live.lock().expect("live lock");
    let l = live.as_ref()?;
    Some((l.rt.clone(), l.game.clone()?))
}

fn stamp_now(game: &mut Game) {
    let now = chrono::Local::now();
    game.info.date = Some(now.format("%Y-%m-%d").to_string());
    game.info.time = Some(now.format("%H:%M").to_string());
}

// ---------- local play and review ----------

#[tauri::command]
fn current(state: State<'_, AppState>) -> View {
    current_view(&state)
}

#[tauri::command]
fn new_game(state: State<'_, AppState>, size: u8, komi: f32) -> Result<View, String> {
    if live_game(&state).is_some_and(|(_, g)| g.phase != "finished") {
        return Err("Finish or resign the live game first".into());
    }
    live::close_game(&state);
    let mut game = Game::new(size.clamp(5, 19), komi);
    stamp_now(&mut game);
    *state.game.lock().expect("game lock") = game;
    Ok(current_view(&state))
}

#[tauri::command]
fn play(state: State<'_, AppState>, x: u8, y: u8) -> Result<View, String> {
    let point = Point::new(x, y);
    if let Some((rt, g)) = live_game(&state) {
        if g.phase != "play" {
            return Err("The game is being scored".into());
        }
        let mut game = state.game.lock().expect("game lock");
        if Some(game.to_move()) != g.my_color {
            return Err("It's your opponent's turn".into());
        }
        // Check legality locally for an instant answer; OGS echoes the move back.
        let mut trial = game.clone();
        trial.go_to(trial.moves().len());
        trial.play(point).map_err(|e| e.to_string())?;
        let total = game.moves().len();
        game.go_to(total);
        drop(game);
        rt.play(g.id, Some(point));
        return Ok(current_view(&state));
    }
    let mut game = state.game.lock().expect("game lock");
    game.play(point).map_err(|e| e.to_string())?;
    drop(game);
    Ok(current_view(&state))
}

#[tauri::command]
fn pass_turn(state: State<'_, AppState>) -> Result<View, String> {
    if let Some((rt, g)) = live_game(&state) {
        let game = state.game.lock().expect("game lock");
        if g.phase != "play" || Some(game.to_move()) != g.my_color {
            return Err("It's your opponent's turn".into());
        }
        drop(game);
        rt.play(g.id, None);
        return Ok(current_view(&state));
    }
    state.game.lock().expect("game lock").pass();
    Ok(current_view(&state))
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Result<View, String> {
    if live_game(&state).is_some_and(|(_, g)| g.phase != "finished") {
        return Err("Take-backs aren't available in live games yet".into());
    }
    state.game.lock().expect("game lock").undo();
    Ok(current_view(&state))
}

#[tauri::command]
fn go_to(state: State<'_, AppState>, n: usize) -> View {
    state.game.lock().expect("game lock").go_to(n);
    current_view(&state)
}

// ---------- files ----------

/// `~/Documents/kaya`, created on first use.
fn records_folder(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().document_dir().ok()?.join("kaya");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[tauri::command]
async fn open_sgf(app: AppHandle, state: State<'_, AppState>) -> Result<Option<View>, String> {
    if live_game(&state).is_some_and(|(_, g)| g.phase != "finished") {
        return Err("Finish or resign the live game first".into());
    }
    let mut dialog = app.dialog().file().add_filter("SGF game record", &["sgf"]);
    if let Some(dir) = records_folder(&app) {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.blocking_pick_file() else { return Ok(None) };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut game = sgf::parse(&text).map_err(|e| e.to_string())?;
    game.go_to(0);
    live::close_game(&state);
    *state.game.lock().expect("game lock") = game;
    Ok(Some(current_view(&state)))
}

/// Saves the game as SGF, offering `~/Documents/kaya` and a descriptive name.
#[tauri::command]
async fn save_sgf(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let (text, name) = {
        let g = state.game.lock().expect("game lock");
        (sgf::write(&g), sgf::suggested_filename(&g))
    };
    let mut dialog = app.dialog().file().set_file_name(&name).add_filter("SGF game record", &["sgf"]);
    if let Some(dir) = records_folder(&app) {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.blocking_save_file() else { return Ok(None) };
    let path = file.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(Some(path.display().to_string()))
}

/// Downloads an OGS game (id or link) and loads it for review.
#[tauri::command]
async fn download_ogs(state: State<'_, AppState>, game: String) -> Result<View, String> {
    if live_game(&state).is_some_and(|(_, g)| g.phase != "finished") {
        return Err("Finish or resign the live game first".into());
    }
    let id = parse_game_ref(&game).ok_or("That doesn't look like an OGS game number or link")?;
    let ogs = state.ogs();
    let text = ogs.download_sgf(&id).await.map_err(|e| e.to_string())?;
    let mut record = sgf::parse(&text).map_err(|e| e.to_string())?;
    record.info.server = Some("OGS".into());
    if let Some(started) = ogs.game_started(&id).await {
        let (date, time) = local_date_time_from_rfc3339(&started);
        record.info.date = date.or(record.info.date);
        record.info.time = time;
    }
    record.go_to(0);
    live::close_game(&state);
    *state.game.lock().expect("game lock") = record;
    Ok(current_view(&state))
}

// ---------- OGS account and live play ----------

#[tauri::command]
fn ogs_status(state: State<'_, AppState>) -> live::OgsStatus {
    live::status(&state)
}

#[tauri::command]
async fn ogs_sign_in(app: AppHandle) -> Result<live::OgsStatus, String> {
    live::sign_in(&app).await?;
    Ok(live::status(&app.state::<AppState>()))
}

#[tauri::command]
fn ogs_sign_out(app: AppHandle, state: State<'_, AppState>) -> live::OgsStatus {
    live::sign_out(&app, &state);
    live::status(&state)
}

#[tauri::command]
fn find_match(app: AppHandle, state: State<'_, AppState>, size: u8, speed: Speed) -> Result<live::OgsStatus, String> {
    live::find_match(&app, &state, size, speed)?;
    Ok(live::status(&state))
}

#[tauri::command]
fn cancel_match(state: State<'_, AppState>) -> live::OgsStatus {
    live::cancel_match(&state);
    live::status(&state)
}

#[tauri::command]
fn resign(state: State<'_, AppState>) -> Result<(), String> {
    let (rt, g) = live_game(&state).ok_or("There's no live game to resign")?;
    rt.resign(g.id);
    Ok(())
}

#[tauri::command]
fn accept_score(state: State<'_, AppState>) -> Result<(), String> {
    let (rt, g) = live_game(&state).ok_or("There's no live game to score")?;
    rt.accept_score(g.id, &g.removed);
    Ok(())
}

#[tauri::command]
fn leave_game(state: State<'_, AppState>) -> View {
    live::close_game(&state);
    current_view(&state)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            game: Mutex::new({
                let mut g = Game::new(19, 6.5);
                stamp_now(&mut g);
                g
            }),
            ogs: Mutex::new(OgsAdapter::default()),
            live: Mutex::new(None),
        })
        .setup(|app| {
            // Reconnect with the sign-in saved in Keychain, if there is one.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { live::resume(&handle).await });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            current, new_game, play, pass_turn, undo, go_to, open_sgf, save_sgf, download_ogs,
            ogs_status, ogs_sign_in, ogs_sign_out, find_match, cancel_match, resign, accept_score, leave_game
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tengen");
}
