//! A game against KataGo playing like a person of a chosen rank. The moves
//! come from `tengen_ai::bot`; this module keeps the game's state, takes the
//! bot's turns in the background and ends the game when it's over.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tengen_ai::bot::{self, Choice, Mood};
use tengen_core::{Color, Game, GameInfo, Move, Point};

use crate::{ai, diag, notify, push_view, AppState};

/// Bumped whenever a pending bot move should be thrown away (undo, a new game).
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub struct BotGame {
    pub you: Color,
    pub rank: String,
    mood: Mood,
    pub over: bool,
    pub thinking: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BotView {
    you: Color,
    rank: String,
    thinking: bool,
    over: bool,
}

pub fn view(state: &AppState) -> Option<BotView> {
    let bot = state.bot.lock().expect("bot lock");
    bot.as_ref().map(|b| BotView {
        you: b.you,
        rank: b.rank.clone(),
        thinking: b.thinking,
        over: b.over,
    })
}

/// True while a game against KataGo is being played.
pub fn in_game(state: &AppState) -> bool {
    state
        .bot
        .lock()
        .expect("bot lock")
        .as_ref()
        .is_some_and(|b| !b.over)
}

/// Forgets the game against KataGo (the record stays on the board).
pub fn stop(state: &AppState) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    *state.bot.lock().expect("bot lock") = None;
}

/// KataGo's name on the board; its rank shows beside it like a player's.
const NAME: &str = "KataGo";

/// Starts a new game against KataGo at `rank`; `you` is your color.
pub fn start(app: &AppHandle, size: u8, komi: f32, you: Color, rank: &str) -> Result<(), String> {
    if bot::profile(rank).is_none() {
        return Err(format!("KataGo doesn't play at {rank}"));
    }
    let state = app.state::<AppState>();
    stop(&state);
    let now = chrono::Local::now();
    let (black, white) = match you {
        Color::Black => ("You".to_string(), NAME.to_string()),
        Color::White => (NAME.to_string(), "You".to_string()),
    };
    let rank_of = |c: Color| (c != you).then(|| rank.to_string());
    let game = Game::from_info(GameInfo {
        size: size.clamp(5, 19),
        komi,
        black: Some(black),
        white: Some(white),
        black_rank: rank_of(Color::Black),
        white_rank: rank_of(Color::White),
        date: Some(now.format("%Y-%m-%d").to_string()),
        time: Some(now.format("%H:%M").to_string()),
        server: Some("KataGo".into()),
        rules: Some("japanese".into()),
        ..Default::default()
    });
    *state.game.lock().expect("game lock") = game;
    *state.bot.lock().expect("bot lock") = Some(BotGame {
        you,
        rank: rank.to_string(),
        mood: Mood::default(),
        over: false,
        thinking: false,
    });
    diag::note(
        "KataGo game",
        format!("started at {rank}, {size}×{size}, you {you:?}"),
    );
    if you == Color::White {
        spawn_turn(app);
    }
    Ok(())
}

/// Your stone in a game against KataGo. None when no such game is on.
pub fn play(app: &AppHandle, point: Point) -> Option<Result<(), String>> {
    your_move(app, Move::Play { point })
}

pub fn pass(app: &AppHandle) -> Option<Result<(), String>> {
    your_move(app, Move::Pass)
}

fn your_move(app: &AppHandle, mv: Move) -> Option<Result<(), String>> {
    let state = app.state::<AppState>();
    let you = {
        let bot = state.bot.lock().expect("bot lock");
        let b = bot.as_ref().filter(|b| !b.over)?;
        if b.thinking {
            return Some(Err("KataGo is thinking".into()));
        }
        b.you
    };
    {
        let mut game = state.game.lock().expect("game lock");
        let total = game.moves().len();
        game.go_to(total);
        if game.to_move() != you {
            return Some(Err("It's KataGo's turn".into()));
        }
        let played = match mv {
            Move::Play { point } => game.play(point).map(|_| ()),
            Move::Pass => {
                game.pass();
                Ok(())
            }
        };
        if let Err(e) = played {
            return Some(Err(e.to_string()));
        }
    }
    spawn_turn(app);
    Some(Ok(()))
}

/// Takes back your last move (and KataGo's answer to it).
pub fn undo(state: &AppState) -> Option<()> {
    let you = state.bot.lock().expect("bot lock").as_ref()?.you;
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut game = state.game.lock().expect("game lock");
    let total = game.moves().len();
    game.go_to(total);
    game.undo();
    if game.to_move() != you && !game.moves().is_empty() {
        game.undo();
    }
    game.info.result = None;
    drop(game);
    if let Some(b) = state.bot.lock().expect("bot lock").as_mut() {
        b.thinking = false;
        b.over = false;
    }
    Some(())
}

/// You resign: KataGo wins.
pub fn resign(app: &AppHandle) -> Option<()> {
    let state = app.state::<AppState>();
    let you = state
        .bot
        .lock()
        .expect("bot lock")
        .as_ref()
        .filter(|b| !b.over)?
        .you;
    GENERATION.fetch_add(1, Ordering::SeqCst);
    finish(
        app,
        format!("{}+R", letter(you.opposite())),
        "KataGo wins by resignation".into(),
    );
    Some(())
}

fn letter(c: Color) -> &'static str {
    if c == Color::Black {
        "B"
    } else {
        "W"
    }
}

fn set_thinking(state: &AppState, on: bool) {
    if let Some(b) = state.bot.lock().expect("bot lock").as_mut() {
        b.thinking = on;
    }
}

fn spawn_turn(app: &AppHandle) {
    let generation = GENERATION.load(Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = turn(&app, generation).await {
            let state = app.state::<AppState>();
            set_thinking(&state, false);
            diag::note("KataGo game", format!("couldn't move: {e}"));
            notify(&app, format!("KataGo couldn't move: {e}"));
            push_view(&app);
        }
    });
}

fn passes_at_end(game: &Game) -> usize {
    game.moves()
        .iter()
        .rev()
        .take_while(|(_, m)| *m == Move::Pass)
        .count()
}

/// After your move: count the game if both passed, otherwise KataGo's turn.
async fn turn(app: &AppHandle, generation: u64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let game = state.game.lock().expect("game lock").clone();
    if passes_at_end(&game) >= 2 {
        return count(app, generation, &game).await;
    }
    let (bot_color, rank, mut mood) = {
        let bot = state.bot.lock().expect("bot lock");
        let Some(b) = bot.as_ref().filter(|b| !b.over) else {
            return Ok(());
        };
        (b.you.opposite(), b.rank.clone(), b.mood.clone())
    };
    if game.to_move() != bot_color {
        return Ok(());
    }
    set_thinking(&state, true);
    push_view(app);
    let started = std::time::Instant::now();
    let engine = ai::engine_with_human(app).await?;
    let choice = bot::choose(&engine, &game, &rank, bot_color, &mut mood, roll())
        .await
        .map_err(|e| e.to_string())?;
    // A moment's pause, so a quick answer still feels like a move.
    let pause = Duration::from_millis(600).saturating_sub(started.elapsed());
    tokio::time::sleep(pause).await;
    if GENERATION.load(Ordering::SeqCst) != generation {
        return Ok(()); // taken back or replaced meanwhile
    }
    {
        let mut g = state.game.lock().expect("game lock");
        if g.moves().len() != game.moves().len() {
            return Ok(());
        }
        let total = g.moves().len();
        g.go_to(total);
        match choice {
            Choice::Play(p) => {
                g.play(p).map_err(|e| e.to_string())?;
            }
            Choice::Pass => g.pass(),
            Choice::Resign => {}
        }
    }
    if let Some(b) = state.bot.lock().expect("bot lock").as_mut() {
        b.mood = mood;
        b.thinking = false;
    }
    match choice {
        Choice::Resign => finish(
            app,
            format!("{}+R", letter(bot_color.opposite())),
            "KataGo resigns. You win".into(),
        ),
        Choice::Pass => {
            let after = state.game.lock().expect("game lock").clone();
            if passes_at_end(&after) >= 2 {
                return count(app, generation, &after).await;
            }
            notify(app, "KataGo passes");
            push_view(app);
        }
        Choice::Play(_) => push_view(app),
    }
    Ok(())
}

/// Both passed: KataGo counts the game.
async fn count(app: &AppHandle, generation: u64, game: &Game) -> Result<(), String> {
    let state = app.state::<AppState>();
    set_thinking(&state, true);
    push_view(app);
    let mut end = game.clone();
    let total = end.moves().len();
    end.go_to(total);
    let estimate = ai::estimate_game(app, &end).await?;
    if GENERATION.load(Ordering::SeqCst) != generation {
        return Ok(());
    }
    let result = bot::result_from_lead(estimate.lead, game.info.komi);
    let you = state.bot.lock().expect("bot lock").as_ref().map(|b| b.you);
    let message = match (result.chars().next(), you) {
        (Some('B'), Some(Color::Black)) | (Some('W'), Some(Color::White)) => {
            format!("You win, {result} by KataGo's count")
        }
        (Some('D'), _) => "A draw, by KataGo's count".to_string(),
        _ => format!("KataGo wins, {result} by its count"),
    };
    finish(app, result, message);
    Ok(())
}

fn finish(app: &AppHandle, result: String, message: String) {
    let state = app.state::<AppState>();
    {
        let mut g = state.game.lock().expect("game lock");
        g.info.result = Some(result.clone());
        let total = g.moves().len();
        g.go_to(total);
    }
    if let Some(b) = state.bot.lock().expect("bot lock").as_mut() {
        b.over = true;
        b.thinking = false;
    }
    diag::note("KataGo game", format!("ended {result}"));
    crate::save_to_library(app);
    notify(app, message);
    push_view(app);
}

/// A random number in [0, 1) for drawing a move.
fn roll() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9);
    let mut x = nanos ^ 0x2545_f491_4f6c_dd1d ^ GENERATION.load(Ordering::Relaxed);
    for _ in 0..4 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    (x >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_passes_at_the_end() {
        let mut g = Game::new(9, 7.0);
        g.play(Point::new(2, 2)).unwrap();
        g.pass();
        assert_eq!(passes_at_end(&g), 1);
        g.pass();
        assert_eq!(passes_at_end(&g), 2);
        g.play(Point::new(4, 4)).unwrap();
        assert_eq!(passes_at_end(&g), 0);
    }

    #[test]
    fn rolls_stay_in_range() {
        for _ in 0..1000 {
            let r = roll();
            assert!((0.0..1.0).contains(&r));
        }
    }
}
