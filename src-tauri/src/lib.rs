//! The Tauri layer: a thin set of commands over `tengen-core` and the
//! server adapters. All game logic lives in the crates, not here.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tengen_core::adapter::ServerAdapter;
use tengen_core::{sgf, Color, Game, Move, Point};
use tengen_ogs::{parse_game_ref, OgsAdapter};

struct AppState {
    game: Mutex<Game>,
    ogs: OgsAdapter,
}

/// Everything the interface needs to draw the board and its edges.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct View {
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
}

fn view(game: &Game) -> View {
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
    }
}

fn with_game<T>(state: &State<'_, AppState>, f: impl FnOnce(&mut Game) -> T) -> T {
    let mut game = state.game.lock().expect("game lock");
    f(&mut game)
}

#[tauri::command]
fn current(state: State<'_, AppState>) -> View {
    with_game(&state, |g| view(g))
}

#[tauri::command]
fn new_game(state: State<'_, AppState>, size: u8, komi: f32) -> View {
    with_game(&state, |g| {
        *g = Game::new(size.clamp(5, 19), komi);
        view(g)
    })
}

#[tauri::command]
fn play(state: State<'_, AppState>, x: u8, y: u8) -> Result<View, String> {
    with_game(&state, |g| {
        g.play(Point::new(x, y)).map_err(|e| e.to_string())?;
        Ok(view(g))
    })
}

#[tauri::command]
fn pass_turn(state: State<'_, AppState>) -> View {
    with_game(&state, |g| {
        g.pass();
        view(g)
    })
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> View {
    with_game(&state, |g| {
        g.undo();
        view(g)
    })
}

#[tauri::command]
fn go_to(state: State<'_, AppState>, n: usize) -> View {
    with_game(&state, |g| {
        g.go_to(n);
        view(g)
    })
}

/// Opens an SGF file chosen in the macOS file dialog.
#[tauri::command]
async fn open_sgf(app: AppHandle, state: State<'_, AppState>) -> Result<Option<View>, String> {
    let Some(file) = app.dialog().file().add_filter("SGF game record", &["sgf"]).blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut game = sgf::parse(&text).map_err(|e| e.to_string())?;
    game.go_to(0);
    Ok(Some(with_game(&state, |g| {
        *g = game;
        view(g)
    })))
}

/// Saves the current game as an SGF file. Returns the saved path.
#[tauri::command]
async fn save_sgf(app: AppHandle, state: State<'_, AppState>) -> Result<Option<String>, String> {
    let (text, name) = with_game(&state, |g| (sgf::write(g), sgf::suggested_filename(g)));
    let Some(file) = app
        .dialog()
        .file()
        .set_file_name(&name)
        .add_filter("SGF game record", &["sgf"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(Some(path.display().to_string()))
}

/// Downloads an OGS game (id or link), loads it for review, and returns it.
#[tauri::command]
async fn download_ogs(state: State<'_, AppState>, game: String) -> Result<View, String> {
    let id = parse_game_ref(&game).ok_or("That doesn't look like an OGS game id or link")?;
    let text = state.ogs.download_sgf(&id).await.map_err(|e| e.to_string())?;
    let mut record = sgf::parse(&text).map_err(|e| e.to_string())?;
    record.info.server.get_or_insert_with(|| "OGS".into());
    record.go_to(0);
    Ok(with_game(&state, |g| {
        *g = record;
        view(g)
    }))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { game: Mutex::new(Game::new(19, 6.5)), ogs: OgsAdapter::default() })
        .invoke_handler(tauri::generate_handler![
            current, new_game, play, pass_turn, undo, go_to, open_sgf, save_sgf, download_ogs
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tengen");
}
