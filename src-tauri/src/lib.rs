//! The Tauri layer: commands the interface calls, plus the live server
//! connections (OGS in `live.rs`, KGS and Pandanet in `sessions.rs`). Game
//! logic lives in the crates; this file only wires them up.

mod ai;
mod keychain;
mod live;
mod sessions;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tengen_core::adapter::{Server, ServerAdapter, Speed};
use tengen_core::{sgf, Color, Game, Move, Point};
use tengen_ogs::realtime::local_date_time_from_rfc3339;
use tengen_ogs::{parse_game_ref, OgsAdapter};

use live::Live;

pub struct AppState {
    pub game: Mutex<Game>,
    pub ogs: Mutex<OgsAdapter>,
    pub live: Mutex<Option<Live>>,
    pub sessions: Mutex<sessions::Sessions>,
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

pub fn view(game: &Game, live: Option<live::LiveView>) -> View {
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
        live,
    }
}

/// The current view, read under the locks.
pub fn current_view(state: &AppState) -> View {
    let other = sessions::live_view(state);
    let live = state.live.lock().expect("live lock");
    let ogs = live.as_ref().and_then(Live::view);
    drop(live);
    let game = state.game.lock().expect("game lock");
    view(&game, ogs.or(other))
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

/// True while a game on any server is being played or scored.
fn in_live_game(state: &AppState) -> bool {
    live::in_game(state) || sessions::in_game(state)
}

/// Stops following live games; the record stays on the board.
fn close_live(state: &AppState) {
    live::close_game(state);
    sessions::close_game(state);
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
    if in_live_game(&state) {
        return Err("Finish or resign the live game first".into());
    }
    close_live(&state);
    let mut game = Game::new(size.clamp(5, 19), komi);
    stamp_now(&mut game);
    *state.game.lock().expect("game lock") = game;
    Ok(current_view(&state))
}

#[tauri::command]
fn play(state: State<'_, AppState>, x: u8, y: u8) -> Result<View, String> {
    let point = Point::new(x, y);
    if let Some((rt, g)) = live_game(&state).filter(|(_, g)| g.phase != "finished") {
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
    if let Some(sent) = sessions::click(&state, point) {
        sent?;
        return Ok(current_view(&state));
    }
    let mut game = state.game.lock().expect("game lock");
    game.play(point).map_err(|e| e.to_string())?;
    drop(game);
    Ok(current_view(&state))
}

#[tauri::command]
fn pass_turn(state: State<'_, AppState>) -> Result<View, String> {
    if let Some((rt, g)) = live_game(&state).filter(|(_, g)| g.phase != "finished") {
        let game = state.game.lock().expect("game lock");
        if g.phase != "play" || Some(game.to_move()) != g.my_color {
            return Err("It's your opponent's turn".into());
        }
        drop(game);
        rt.play(g.id, None);
        return Ok(current_view(&state));
    }
    if let Some(sent) = sessions::pass(&state) {
        sent?;
        return Ok(current_view(&state));
    }
    state.game.lock().expect("game lock").pass();
    Ok(current_view(&state))
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Result<View, String> {
    if in_live_game(&state) {
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
    if in_live_game(&state) {
        return Err("Finish or resign the live game first".into());
    }
    let mut dialog = app.dialog().file().add_filter("SGF game record", &["sgf"]);
    if let Some(dir) = records_folder(&app) {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.blocking_pick_file() else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut game = sgf::parse(&text).map_err(|e| e.to_string())?;
    game.go_to(0);
    close_live(&state);
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
    let mut dialog = app
        .dialog()
        .file()
        .set_file_name(&name)
        .add_filter("SGF game record", &["sgf"]);
    if let Some(dir) = records_folder(&app) {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(Some(path.display().to_string()))
}

/// Downloads an OGS game (id or link) and loads it for review.
#[tauri::command]
async fn download_ogs(state: State<'_, AppState>, game: String) -> Result<View, String> {
    if in_live_game(&state) {
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
    close_live(&state);
    *state.game.lock().expect("game lock") = record;
    Ok(current_view(&state))
}

// ---------- accounts and live play ----------

#[tauri::command]
fn server_status(state: State<'_, AppState>) -> Vec<sessions::ServerStatus> {
    sessions::statuses(&state)
}

#[tauri::command]
async fn ogs_sign_in(app: AppHandle) -> Result<Vec<sessions::ServerStatus>, String> {
    live::sign_in(&app).await?;
    Ok(sessions::statuses(&app.state::<AppState>()))
}

/// Signs in to KGS or Pandanet; the login is kept in Keychain.
#[tauri::command]
async fn server_sign_in(
    app: AppHandle,
    server: Server,
    username: String,
    password: String,
) -> Result<Vec<sessions::ServerStatus>, String> {
    if !sessions::PASSWORD_SERVERS.contains(&server) {
        return Err(format!("{} doesn't sign in with a password", server.name()));
    }
    sessions::sign_in(&app, server, username, password).await?;
    Ok(sessions::statuses(&app.state::<AppState>()))
}

#[tauri::command]
fn server_sign_out(
    app: AppHandle,
    state: State<'_, AppState>,
    server: Server,
) -> Vec<sessions::ServerStatus> {
    match server {
        Server::Ogs => live::sign_out(&app, &state),
        other => sessions::sign_out(&app, other),
    }
    sessions::statuses(&state)
}

#[tauri::command]
fn find_match(
    app: AppHandle,
    state: State<'_, AppState>,
    server: Server,
    size: u8,
    speed: Speed,
) -> Result<Vec<sessions::ServerStatus>, String> {
    match server {
        Server::Ogs => live::find_match(&app, &state, size, speed)?,
        other => sessions::find_match(&app, other, size, speed)?,
    }
    Ok(sessions::statuses(&state))
}

#[tauri::command]
fn cancel_match(state: State<'_, AppState>) -> Vec<sessions::ServerStatus> {
    live::cancel_match(&state);
    sessions::cancel_match(&state);
    sessions::statuses(&state)
}

#[tauri::command]
fn resign(state: State<'_, AppState>) -> Result<(), String> {
    if let Some((rt, g)) = live_game(&state).filter(|(_, g)| g.phase != "finished") {
        rt.resign(g.id);
        return Ok(());
    }
    sessions::resign(&state).ok_or_else(|| "There's no live game to resign".to_string())
}

#[tauri::command]
fn accept_score(state: State<'_, AppState>) -> Result<(), String> {
    if let Some((rt, g)) = live_game(&state).filter(|(_, g)| g.phase == "stone removal") {
        rt.accept_score(g.id, &g.removed);
        return Ok(());
    }
    sessions::accept_score(&state).ok_or_else(|| "There's no game being scored".to_string())
}

// ---------- score estimate and AI review ----------

#[tauri::command]
fn ai_status(app: AppHandle) -> ai::AiStatus {
    ai::status(&app)
}

#[tauri::command]
async fn ai_install(app: AppHandle) -> Result<ai::AiStatus, String> {
    ai::install(&app).await
}

/// Allowed in your own live games too: it shows score and territory, never moves.
#[tauri::command]
async fn estimate_score(app: AppHandle) -> Result<tengen_ai::estimate::Estimate, String> {
    ai::estimate(&app).await
}

/// Which side you played, when Tengen can tell from the live game or your OGS name.
#[tauri::command]
fn review_side(app: AppHandle) -> Option<Color> {
    ai::guess_you(&app)
}

#[tauri::command]
async fn start_review(
    app: AppHandle,
    state: State<'_, AppState>,
    you: Color,
) -> Result<ai::ReviewView, String> {
    if in_live_game(&state) {
        return Err("Reviews open once the game is over".into());
    }
    ai::review(&app, you).await
}

#[tauri::command]
fn set_claude_key(app: AppHandle, key: String) -> Result<ai::AiStatus, String> {
    let key = key.trim();
    if !key.starts_with("sk-ant-") {
        return Err("That doesn't look like a Claude API key (they start with sk-ant-)".into());
    }
    keychain::save_claude_key(key)?;
    Ok(ai::status(&app))
}

#[tauri::command]
fn clear_claude_key(app: AppHandle) -> ai::AiStatus {
    keychain::forget_claude_key();
    ai::status(&app)
}

#[tauri::command]
fn list_maxims(app: AppHandle) -> Vec<(String, Vec<String>)> {
    ai::maxims(&app)
}

#[tauri::command]
fn leave_game(state: State<'_, AppState>) -> View {
    close_live(&state);
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
            sessions: Mutex::new(sessions::Sessions::default()),
        })
        .manage(ai::AiState::default())
        .setup(|app| {
            // Reconnect with the sign-in saved in Keychain, if there is one.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                live::resume(&handle).await;
                sessions::resume(&handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            current,
            new_game,
            play,
            pass_turn,
            undo,
            go_to,
            open_sgf,
            save_sgf,
            download_ogs,
            server_status,
            ogs_sign_in,
            server_sign_in,
            server_sign_out,
            find_match,
            cancel_match,
            resign,
            accept_score,
            leave_game,
            ai_status,
            ai_install,
            estimate_score,
            review_side,
            start_review,
            set_claude_key,
            clear_claude_key,
            list_maxims
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tengen");
}
