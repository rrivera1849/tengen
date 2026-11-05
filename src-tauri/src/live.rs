//! The OGS account and live connection: sign-in, automatch, and turning
//! what OGS says during a game into updates of the board.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use tengen_core::adapter::{Account, AdapterError, Speed};
use tengen_core::{Color, Move};
use tengen_ogs::auth::{self, Pkce};
use tengen_ogs::realtime::{decode_points, GameData, OgsEvent, Realtime};
use tokio::sync::mpsc;

use crate::{diag, keychain, notify, push_view, AppState};

/// Bumped on every new connection, so events from an old one are ignored.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub struct Live {
    pub rt: Realtime,
    pub me: Account,
    pub connected: bool,
    pub searching: Option<String>,
    pub game: Option<LiveGame>,
    generation: u64,
}

#[derive(Clone)]
pub struct LiveGame {
    pub id: u64,
    pub my_color: Option<Color>,
    pub phase: String,
    /// Dead stones OGS proposes once both players pass, packed like `pdpp`.
    pub removed: String,
    size: u8,
    black_id: u64,
    white_id: u64,
    /// With free handicap placement black's first stones arrive as moves.
    handicap_moves: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LiveView {
    game_id: u64,
    my_color: Option<Color>,
    phase: String,
    removed: Vec<(u8, u8)>,
    /// True while scoring on a server where you click dead stones yourself.
    mark_dead: bool,
}

impl LiveView {
    pub fn new(
        game_id: u64,
        my_color: Option<Color>,
        phase: &str,
        removed: Vec<(u8, u8)>,
        mark_dead: bool,
    ) -> Self {
        LiveView {
            game_id,
            my_color,
            phase: phase.to_string(),
            removed,
            mark_dead,
        }
    }
}

impl Live {
    pub fn view(&self) -> Option<LiveView> {
        let g = self.game.as_ref()?;
        Some(LiveView {
            game_id: g.id,
            my_color: g.my_color,
            phase: g.phase.clone(),
            removed: decode_points(&g.removed, g.size)
                .into_iter()
                .map(|p| (p.x, p.y))
                .collect(),
            mark_dead: false,
        })
    }

    fn my_id(&self) -> u64 {
        self.me.id.parse().unwrap_or(0)
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OgsStatus {
    /// False until Tengen is registered with OGS (see the README).
    pub(crate) configured: bool,
    pub(crate) signed_in: bool,
    pub(crate) connected: bool,
    pub(crate) username: Option<String>,
    pub(crate) rank: Option<String>,
    pub(crate) searching: bool,
    pub(crate) in_game: bool,
}

pub fn status(state: &AppState) -> OgsStatus {
    let signed_in = state.ogs.lock().expect("ogs lock").is_signed_in();
    let live = state.live.lock().expect("live lock");
    OgsStatus {
        configured: auth::client_id().is_some(),
        signed_in,
        connected: live.as_ref().is_some_and(|l| l.connected),
        username: live.as_ref().map(|l| l.me.username.clone()),
        rank: live.as_ref().and_then(|l| l.me.rank.clone()),
        searching: live.as_ref().is_some_and(|l| l.searching.is_some()),
        in_game: live
            .as_ref()
            .is_some_and(|l| l.game.as_ref().is_some_and(|g| g.phase != "finished")),
    }
}

fn push_status(app: &AppHandle) {
    crate::sessions::push_status(app);
}

/// True while an OGS game is being played or scored.
pub fn in_game(state: &AppState) -> bool {
    let live = state.live.lock().expect("live lock");
    live.as_ref()
        .and_then(|l| l.game.as_ref())
        .is_some_and(|g| g.phase != "finished")
}

// ---------- signing in ----------

pub async fn sign_in(app: &AppHandle) -> Result<(), String> {
    let client_id = auth::client_id()
        .ok_or("Tengen isn't registered with OGS yet. See \"Signing in to OGS\" in the README.")?;
    let ogs = app.state::<AppState>().ogs();
    let pkce = Pkce::new();
    let url = auth::authorize_url(ogs.base_url(), client_id, &pkce);
    // Start listening before the browser opens, so the reply can't be missed.
    let waiting = tokio::spawn({
        let state = pkce.state.clone();
        async move { auth::wait_for_code(&state, Duration::from_secs(300)).await }
    });
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("couldn't open the browser: {e}"))?;
    let code = waiting
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| match e {
            AdapterError::BadCredentials => "Sign-in was cancelled on OGS".to_string(),
            other => other.to_string(),
        })?;
    let tokens = auth::exchange_code(ogs.http(), ogs.base_url(), client_id, &code, &pkce.verifier)
        .await
        .map_err(|e| e.to_string())?;
    keychain::save_ogs(&tokens)?;
    set_token(app, Some(tokens.access_token));
    connect(app).await
}

/// On launch: reuse the sign-in saved in Keychain, renewing it if it's old.
pub async fn resume(app: &AppHandle) {
    let Some(mut tokens) = keychain::load_ogs() else {
        return push_status(app);
    };
    if tokens.needs_refresh() {
        let (Some(client_id), ogs) = (auth::client_id(), app.state::<AppState>().ogs()) else {
            return;
        };
        match auth::refresh(ogs.http(), ogs.base_url(), client_id, &tokens.refresh_token).await {
            Ok(t) => {
                let _ = keychain::save_ogs(&t);
                tokens = t;
            }
            Err(AdapterError::BadCredentials) => {
                keychain::forget_ogs();
                notify(app, "Your OGS sign-in expired. Sign in again to play.");
                return push_status(app);
            }
            // Offline at launch: keep the old token and try anyway.
            Err(_) => {}
        }
    }
    set_token(app, Some(tokens.access_token));
    if let Err(e) = connect(app).await {
        notify(app, e);
    }
}

pub fn sign_out(app: &AppHandle, state: &AppState) {
    keychain::forget_ogs();
    set_token(app, None);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    *state.live.lock().expect("live lock") = None; // closes the socket
    push_status(app);
    push_view(app);
}

fn set_token(app: &AppHandle, token: Option<String>) {
    app.state::<AppState>()
        .ogs
        .lock()
        .expect("ogs lock")
        .set_token(token);
}

// ---------- the connection ----------

/// Boxed with an explicit `Send`, because reconnecting calls back into it.
fn connect(app: &AppHandle) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
    Box::pin(async move { connect_now(app).await })
}

async fn connect_now(app: &AppHandle) -> Result<(), String> {
    diag::note("OGS", "connecting");
    let state = app.state::<AppState>();
    let ogs = state.ogs();
    let (me, jwt) = ogs.live_credentials().await.map_err(|e| match e {
        AdapterError::NotSignedIn => {
            "OGS didn't accept the saved sign-in. Sign in again.".to_string()
        }
        other => other.to_string(),
    })?;
    let (tx, rx) = mpsc::unbounded_channel();
    let rt = Realtime::connect(ogs.base_url(), &jwt, tx)
        .await
        .map_err(|e| {
            diag::note("OGS", format!("couldn't connect: {e}"));
            e.to_string()
        })?;
    diag::note("OGS", format!("connected as {}", me.username));
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut live = state.live.lock().expect("live lock");
        // After a reconnect, rejoin the game that was open.
        let game = live.take().and_then(|l| l.game);
        if let Some(g) = &game {
            rt.join_game(g.id);
        }
        *live = Some(Live {
            rt,
            me,
            connected: true,
            searching: None,
            game,
            generation,
        });
    }
    tauri::async_runtime::spawn(listen(app.clone(), rx, generation));
    push_status(app);
    Ok(())
}

async fn listen(app: AppHandle, mut rx: mpsc::UnboundedReceiver<OgsEvent>, generation: u64) {
    while let Some(event) = rx.recv().await {
        if GENERATION.load(Ordering::SeqCst) != generation {
            return; // a newer connection took over
        }
        if let OgsEvent::Disconnected { reason } = event {
            return reconnect(app, generation, reason).await;
        }
        handle(&app, event);
    }
}

async fn reconnect(app: AppHandle, generation: u64, reason: String) {
    diag::note("OGS", format!("connection dropped: {reason}"));
    {
        let state = app.state::<AppState>();
        let mut live = state.live.lock().expect("live lock");
        match live.as_mut() {
            Some(l) if l.generation == generation => {
                l.connected = false;
                l.searching = None;
            }
            _ => return,
        }
    }
    push_status(&app);
    notify(&app, "Lost the connection to OGS. Reconnecting");
    for wait in [2, 4, 8, 15, 30] {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        if GENERATION.load(Ordering::SeqCst) != generation
            || !app.state::<AppState>().ogs().is_signed_in()
        {
            return;
        }
        if connect(&app).await.is_ok() {
            return notify(&app, "Reconnected to OGS");
        }
    }
    notify(
        &app,
        format!("Couldn't reconnect to OGS ({reason}). Check your connection."),
    );
}

fn handle(app: &AppHandle, event: OgsEvent) {
    let state = app.state::<AppState>();
    match event {
        OgsEvent::Authenticated { .. } => push_status(app),

        OgsEvent::MatchStarted { uuid, game_id } => {
            diag::note("OGS", format!("match started: game {game_id}"));
            {
                let mut live = state.live.lock().expect("live lock");
                let Some(l) = live.as_mut() else { return };
                if l.searching.as_deref() != Some(uuid.as_str()) {
                    return; // a request from another device
                }
                l.searching = None;
                crate::sessions::close_game(&state);
                crate::bot::stop(&state);
                l.game = Some(LiveGame {
                    id: game_id,
                    my_color: None,
                    phase: "play".into(),
                    removed: String::new(),
                    size: 19,
                    black_id: 0,
                    white_id: 0,
                    handicap_moves: 0,
                });
                l.rt.join_game(game_id);
            }
            notify(app, "Opponent found");
            push_status(app);
        }

        OgsEvent::MatchCanceled { .. } => {
            if let Some(l) = state.live.lock().expect("live lock").as_mut() {
                l.searching = None;
            }
            push_status(app);
        }

        OgsEvent::GameData { game: data } => {
            let finished = data.is_finished();
            {
                let mut live = state.live.lock().expect("live lock");
                let Some(l) = live.as_mut() else { return };
                let me = l.my_id();
                let Some(g) = l.game.as_mut().filter(|g| g.id == data.game_id) else {
                    return;
                };
                g.my_color = data.color_of(me);
                g.phase = data.phase.clone().unwrap_or_else(|| "play".into());
                g.removed = removed_string(&data);
                g.size = data.width;
                g.black_id = data.players.black.id;
                g.white_id = data.players.white.id;
                g.handicap_moves = match (data.handicap, data.free_handicap_placement) {
                    (Some(h), Some(true)) if h > 1 => h as usize,
                    _ => 0,
                };
                let mut record = data.to_game();
                record.go_to(record.moves().len());
                *state.game.lock().expect("game lock") = record;
            }
            push_view(app);
            push_status(app);
            if finished {
                announce_result(app);
            }
        }

        OgsEvent::Moved { game_id, point, .. } => {
            {
                let live = state.live.lock().expect("live lock");
                let Some(g) = live
                    .as_ref()
                    .and_then(|l| l.game.as_ref())
                    .filter(|g| g.id == game_id)
                else {
                    return;
                };
                let mut game = state.game.lock().expect("game lock");
                let total = game.moves().len();
                game.go_to(total);
                let color = if total < g.handicap_moves {
                    Color::Black
                } else {
                    game.to_move()
                };
                game.play_record(
                    color,
                    point.map_or(Move::Pass, |point| Move::Play { point }),
                );
            }
            push_view(app);
        }

        OgsEvent::Clock { clock } => {
            let open = state
                .live
                .lock()
                .expect("live lock")
                .as_ref()
                .and_then(|l| l.game.as_ref())
                .map(|g| g.id);
            if open == Some(clock.game_id) {
                let _ = app.emit("clock", clock);
            }
        }

        OgsEvent::Phase { game_id, phase } => {
            diag::note("OGS", format!("game {game_id} phase {phase}"));
            let entering_scoring = phase == "stone removal";
            if !set_game(&state, game_id, |g| g.phase = phase) {
                return;
            }
            push_view(app);
            push_status(app);
            if entering_scoring {
                notify(
                    app,
                    "Both passed. Check the dead stones, then accept the score",
                );
            }
        }

        OgsEvent::RemovedStones {
            game_id,
            all_removed,
        } => {
            if set_game(&state, game_id, |g| g.removed = all_removed) {
                push_view(app);
            }
        }

        OgsEvent::ScoreAccepted {
            game_id,
            phase,
            outcome,
            winner,
            ..
        } => {
            let mut winner_color = None;
            let ok = set_game(&state, game_id, |g| {
                g.phase = phase.clone();
                winner_color = winner.and_then(|w| {
                    (w == g.black_id)
                        .then_some(Color::Black)
                        .or((w == g.white_id).then_some(Color::White))
                });
            });
            if !ok {
                return;
            }
            if phase == "finished" {
                if let (Some(c), Some(o)) = (winner_color, outcome) {
                    let letter = if c == Color::Black { "B" } else { "W" };
                    state.game.lock().expect("game lock").info.result =
                        Some(format!("{letter}+{}", o.trim_end_matches(" points")));
                }
                announce_result(app);
            }
            push_view(app);
            push_status(app);
        }

        OgsEvent::GameError { message, .. } | OgsEvent::Error { message } => notify(app, message),

        OgsEvent::Disconnected { .. } => {}
    }
}

/// Applies `f` to the open live game if it is `game_id`. Returns whether it was.
fn set_game(state: &AppState, game_id: u64, f: impl FnOnce(&mut LiveGame)) -> bool {
    let mut live = state.live.lock().expect("live lock");
    match live
        .as_mut()
        .and_then(|l| l.game.as_mut())
        .filter(|g| g.id == game_id)
    {
        Some(g) => {
            f(g);
            true
        }
        None => false,
    }
}

fn removed_string(data: &GameData) -> String {
    match &data.removed {
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn announce_result(app: &AppHandle) {
    crate::save_to_library(app);
    let state = app.state::<AppState>();
    let (result, mine) = {
        let game = state.game.lock().expect("game lock");
        let live = state.live.lock().expect("live lock");
        let mine = live
            .as_ref()
            .and_then(|l| l.game.as_ref())
            .and_then(|g| g.my_color);
        (game.info.result.clone(), mine)
    };
    let Some(result) = result else { return };
    let won = match (result.chars().next(), mine) {
        (Some('B'), Some(Color::Black)) | (Some('W'), Some(Color::White)) => Some(true),
        (Some('B'), Some(Color::White)) | (Some('W'), Some(Color::Black)) => Some(false),
        _ => None,
    };
    notify(
        app,
        match won {
            Some(true) => format!("You won, {result}"),
            Some(false) => format!("You lost, {result}"),
            None => format!("Game over, {result}"),
        },
    );
}

// ---------- automatch ----------

pub fn find_match(app: &AppHandle, state: &AppState, size: u8, speed: Speed) -> Result<(), String> {
    let mut live = state.live.lock().expect("live lock");
    let l = live
        .as_mut()
        .filter(|l| l.connected)
        .ok_or("Sign in to OGS to play online")?;
    if l.game.as_ref().is_some_and(|g| g.phase != "finished") || crate::sessions::in_game(state) {
        return Err("You're already in a game".into());
    }
    if let Some(old) = l.searching.take() {
        l.rt.cancel_match(&old);
    }
    l.searching = Some(l.rt.find_match(size, speed));
    diag::note("OGS", format!("looking for a {size}×{size} {speed:?} game"));
    drop(live);
    push_status(app);
    Ok(())
}

pub fn cancel_match(state: &AppState) {
    if let Some(l) = state.live.lock().expect("live lock").as_mut() {
        if let Some(uuid) = l.searching.take() {
            l.rt.cancel_match(&uuid);
        }
    }
}

/// Stops following the live game; its record stays on the board for review.
pub fn close_game(state: &AppState) {
    if let Some(l) = state.live.lock().expect("live lock").as_mut() {
        if let Some(g) = l.game.take() {
            l.rt.leave_game(g.id);
        }
    }
}
