//! KGS and Pandanet: password sign-in, automatch and live play through the
//! shared [`LiveClient`] interface. OGS has its own module (`live.rs`),
//! because it signs in through the browser and was built first.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tengen_core::adapter::{Account, AdapterError, Credentials, Server, Speed};
use tengen_core::live::{LiveClient, LiveEvent, LiveGame, Phase};
use tengen_core::{Color, Game, GameInfo, Move, Point};
use tokio::sync::mpsc;

use crate::{keychain, live, notify, push_view, AppState};

/// The servers that sign in with a name and password.
pub const PASSWORD_SERVERS: [Server; 3] = [Server::Kgs, Server::Igs, Server::Fox];

static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct Sessions {
    map: HashMap<Server, Session>,
    /// The server whose game is on the board, if a live game is open.
    active: Option<Server>,
}

pub struct Session {
    client: Option<Arc<dyn LiveClient>>,
    me: Option<Account>,
    connected: bool,
    searching: bool,
    game: Option<SessionGame>,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct SessionGame {
    pub id: u64,
    pub my_color: Option<Color>,
    pub phase: Phase,
    pub dead: Vec<Point>,
}

/// One server's state, as the interface shows it.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    server: Server,
    /// False for OGS until Tengen is registered there (see the README).
    configured: bool,
    signed_in: bool,
    connected: bool,
    username: Option<String>,
    rank: Option<String>,
    searching: bool,
    in_game: bool,
}

pub fn statuses(state: &AppState) -> Vec<ServerStatus> {
    let ogs = live::status(state);
    let mut all = vec![ServerStatus {
        server: Server::Ogs,
        configured: ogs.configured,
        signed_in: ogs.signed_in,
        connected: ogs.connected,
        username: ogs.username,
        rank: ogs.rank,
        searching: ogs.searching,
        in_game: ogs.in_game,
    }];
    let sessions = state.sessions.lock().expect("sessions lock");
    for server in PASSWORD_SERVERS {
        let s = sessions.map.get(&server);
        all.push(ServerStatus {
            server,
            configured: true,
            signed_in: s.is_some(),
            connected: s.is_some_and(|s| s.connected),
            username: s.and_then(|s| s.me.as_ref().map(|m| m.username.clone())),
            rank: s.and_then(|s| s.me.as_ref().and_then(|m| m.rank.clone())),
            searching: s.is_some_and(|s| s.searching),
            in_game: s.is_some_and(|s| s.game.as_ref().is_some_and(|g| g.phase != Phase::Finished)),
        });
    }
    all
}

pub fn push_status(app: &AppHandle) {
    let _ = app.emit("servers", statuses(&app.state::<AppState>()));
}

// ---------- the open game ----------

/// The live game on the board, for the interface.
pub fn live_view(state: &AppState) -> Option<live::LiveView> {
    let sessions = state.sessions.lock().expect("sessions lock");
    let server = sessions.active?;
    let session = sessions.map.get(&server)?;
    let g = session.game.as_ref()?;
    let marks = session
        .client
        .as_ref()
        .is_none_or(|c| c.marks_dead_stones());
    Some(live::LiveView::new(
        g.id,
        g.my_color,
        match g.phase {
            Phase::Play => "play",
            Phase::Scoring => "stone removal",
            Phase::Finished => "finished",
        },
        g.dead.iter().map(|p| (p.x, p.y)).collect(),
        g.phase == Phase::Scoring && marks,
    ))
}

/// The open game and the connection it belongs to.
pub fn active(state: &AppState) -> Option<(Arc<dyn LiveClient>, SessionGame)> {
    let sessions = state.sessions.lock().expect("sessions lock");
    let s = sessions.map.get(&sessions.active?)?;
    Some((s.client.clone()?, s.game.clone()?))
}

/// True while a game on KGS or Pandanet is being played or scored.
pub fn in_game(state: &AppState) -> bool {
    let sessions = state.sessions.lock().expect("sessions lock");
    sessions
        .active
        .and_then(|a| sessions.map.get(&a))
        .and_then(|s| s.game.as_ref())
        .is_some_and(|g| g.phase != Phase::Finished)
}

/// Stops following the open game; its record stays on the board.
pub fn close_game(state: &AppState) {
    let mut sessions = state.sessions.lock().expect("sessions lock");
    let Some(server) = sessions.active.take() else {
        return;
    };
    if let Some(s) = sessions.map.get_mut(&server) {
        if let (Some(g), Some(c)) = (s.game.take(), &s.client) {
            c.leave_game(g.id);
        }
    }
}

/// The player's name on any server they're signed in to (for guessing sides).
pub fn usernames(state: &AppState) -> Vec<String> {
    let sessions = state.sessions.lock().expect("sessions lock");
    sessions
        .map
        .values()
        .filter_map(|s| s.me.as_ref().map(|m| m.username.to_lowercase()))
        .collect()
}

pub fn my_color(state: &AppState) -> Option<Color> {
    let sessions = state.sessions.lock().expect("sessions lock");
    sessions
        .active
        .and_then(|a| sessions.map.get(&a))
        .and_then(|s| s.game.as_ref())
        .and_then(|g| g.my_color)
}

// ---------- signing in ----------

pub async fn sign_in(
    app: &AppHandle,
    server: Server,
    username: String,
    password: String,
) -> Result<(), String> {
    let credentials = Credentials {
        username: username.trim().to_string(),
        password,
    };
    if credentials.username.is_empty() || credentials.password.is_empty() {
        return Err("Enter your name and password".into());
    }
    connect(app, server, &credentials)
        .await
        .map_err(|e| match e {
            AdapterError::BadCredentials => {
                format!("{} didn't accept that name and password", label(server))
            }
            other => other.to_string(),
        })?;
    keychain::save_login(server, &credentials)?;
    Ok(())
}

/// On launch: sign in again with logins saved in Keychain.
pub async fn resume(app: &AppHandle) {
    for server in PASSWORD_SERVERS {
        if let Some(credentials) = keychain::load_login(server) {
            if let Err(e) = connect(app, server, &credentials).await {
                notify(app, format!("Couldn't sign in to {}: {e}", label(server)));
            }
        }
    }
    push_status(app);
}

pub fn sign_out(app: &AppHandle, server: Server) {
    keychain::forget_login(server);
    let state = app.state::<AppState>();
    {
        let mut sessions = state.sessions.lock().expect("sessions lock");
        if let Some(s) = sessions.map.remove(&server) {
            if let Some(c) = s.client {
                c.close();
            }
        }
        if sessions.active == Some(server) {
            sessions.active = None;
        }
    }
    push_status(app);
    push_view(app);
}

pub fn label(server: Server) -> &'static str {
    match server {
        Server::Igs => "Pandanet",
        other => other.name(),
    }
}

/// Boxed with an explicit `Send`, because reconnecting calls back into it.
fn connect<'a>(
    app: &'a AppHandle,
    server: Server,
    credentials: &'a Credentials,
) -> Pin<Box<dyn Future<Output = Result<(), AdapterError>> + Send + 'a>> {
    Box::pin(connect_now(app, server, credentials))
}

async fn connect_now(
    app: &AppHandle,
    server: Server,
    credentials: &Credentials,
) -> Result<(), AdapterError> {
    let (tx, rx) = mpsc::unbounded_channel();
    let client: Arc<dyn LiveClient> = match server {
        Server::Kgs => Arc::new(tengen_kgs::connect(tengen_kgs::ENDPOINT, credentials, tx).await?),
        Server::Igs => Arc::new(
            tengen_igs::connect(tengen_igs::HOST, tengen_igs::PORT, credentials, tx).await?,
        ),
        Server::Fox => Arc::new(tengen_fox::connect(credentials, tx).await?),
        _ => return Err(AdapterError::Unsupported),
    };
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let state = app.state::<AppState>();
        let mut sessions = state.sessions.lock().expect("sessions lock");
        let old = sessions.map.remove(&server);
        if let Some(c) = old.as_ref().and_then(|o| o.client.clone()) {
            c.close();
        }
        // Keep the open game across a reconnect; the server sends it again.
        let game = old.and_then(|o| o.game);
        sessions.map.insert(
            server,
            Session {
                client: Some(client),
                me: None,
                connected: true,
                searching: false,
                game,
                generation,
            },
        );
    }
    tauri::async_runtime::spawn(listen(app.clone(), server, rx, generation));
    push_status(app);
    Ok(())
}

async fn listen(
    app: AppHandle,
    server: Server,
    mut rx: mpsc::UnboundedReceiver<LiveEvent>,
    generation: u64,
) {
    while let Some(event) = rx.recv().await {
        if current_generation(&app, server) != Some(generation) {
            return; // signed out, or a newer connection took over
        }
        if let LiveEvent::Disconnected { reason, retry } = event {
            return dropped(app, server, generation, reason, retry).await;
        }
        handle(&app, server, event);
    }
}

fn current_generation(app: &AppHandle, server: Server) -> Option<u64> {
    let state = app.state::<AppState>();
    let sessions = state.sessions.lock().expect("sessions lock");
    sessions.map.get(&server).map(|s| s.generation)
}

async fn dropped(app: AppHandle, server: Server, generation: u64, reason: String, retry: bool) {
    {
        let state = app.state::<AppState>();
        let mut sessions = state.sessions.lock().expect("sessions lock");
        let Some(s) = sessions
            .map
            .get_mut(&server)
            .filter(|s| s.generation == generation)
        else {
            return;
        };
        s.connected = false;
        s.searching = false;
        s.client = None;
    }
    push_status(&app);
    let name = label(server);
    if !retry {
        return notify(
            &app,
            format!("{reason}. Sign in to {name} again from Online to play here."),
        );
    }
    notify(&app, format!("Lost the connection to {name}. Reconnecting"));
    for wait in [2, 4, 8, 15, 30, 60] {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        if current_generation(&app, server) != Some(generation) {
            return; // signed out meanwhile
        }
        let Some(credentials) = keychain::load_login(server) else {
            return;
        };
        match connect(&app, server, &credentials).await {
            Ok(()) => return notify(&app, format!("Reconnected to {name}")),
            Err(AdapterError::BadCredentials) => break,
            Err(_) => {}
        }
    }
    notify(
        &app,
        format!("Couldn't reconnect to {name} ({reason}). Check your connection."),
    );
}

fn with_session<T>(
    app: &AppHandle,
    server: Server,
    f: impl FnOnce(&mut Session) -> T,
) -> Option<T> {
    let state = app.state::<AppState>();
    let mut sessions = state.sessions.lock().expect("sessions lock");
    sessions.map.get_mut(&server).map(f)
}

/// Runs `f` on the session's game when it is game `id`. Returns whether it was.
fn with_game(app: &AppHandle, server: Server, id: u64, f: impl FnOnce(&mut SessionGame)) -> bool {
    with_session(app, server, |s| {
        match s.game.as_mut().filter(|g| g.id == id) {
            Some(g) => {
                f(g);
                true
            }
            None => false,
        }
    })
    .unwrap_or(false)
}

fn handle(app: &AppHandle, server: Server, event: LiveEvent) {
    let state = app.state::<AppState>();
    match event {
        LiveEvent::Ready { account } => {
            with_session(app, server, |s| {
                s.me = Some(account);
                s.connected = true;
            });
            push_status(app);
        }

        LiveEvent::SearchStopped => {
            with_session(app, server, |s| s.searching = false);
            push_status(app);
        }

        LiveEvent::Game(g) => open_game(app, server, g),

        LiveEvent::Moved { game_id, color, mv } => {
            if !is_on_board(&state, server, game_id) {
                return;
            }
            {
                let mut game = state.game.lock().expect("game lock");
                let total = game.moves().len();
                game.go_to(total);
                game.play_record(color, mv);
            }
            push_view(app);
        }

        LiveEvent::Clock(clock) => {
            if is_on_board(&state, server, clock.game_id) {
                let _ = app.emit("clock", clock);
            }
        }

        LiveEvent::Phase { game_id, phase } => {
            let mut changed = false;
            with_game(app, server, game_id, |g| {
                changed = g.phase != phase;
                g.phase = phase;
                if phase != Phase::Scoring {
                    g.dead.clear();
                }
            });
            if !changed {
                return;
            }
            push_view(app);
            push_status(app);
            let marks = with_session(app, server, |s| {
                s.client.as_ref().is_none_or(|c| c.marks_dead_stones())
            })
            .unwrap_or(true);
            if phase == Phase::Scoring && marks && is_on_board(&state, server, game_id) {
                notify(
                    app,
                    "Both passed. Click dead stones to mark them, then accept the score",
                );
            }
        }

        LiveEvent::Dead { game_id, points } => {
            if with_game(app, server, game_id, |g| g.dead = points) {
                push_view(app);
            }
        }

        LiveEvent::Ended { game_id, result } => {
            let mut mine = None;
            if !with_game(app, server, game_id, |g| {
                g.phase = Phase::Finished;
                mine = g.my_color;
            }) {
                return;
            }
            if is_on_board(&state, server, game_id) {
                state.game.lock().expect("game lock").info.result = Some(result.clone());
                announce(app, &result, mine);
            }
            push_view(app);
            push_status(app);
        }

        LiveEvent::Notice(text) => notify(app, text),

        LiveEvent::Disconnected { .. } => {}
    }
}

fn is_on_board(state: &AppState, server: Server, game_id: u64) -> bool {
    let sessions = state.sessions.lock().expect("sessions lock");
    sessions.active == Some(server)
        && sessions
            .map
            .get(&server)
            .and_then(|s| s.game.as_ref())
            .is_some_and(|g| g.id == game_id)
}

/// Puts a game from the server on the board, or brings the board up to date.
fn open_game(app: &AppHandle, server: Server, g: LiveGame) {
    let state = app.state::<AppState>();
    if live::in_game(&state) {
        notify(
            app,
            format!(
                "A {} game started while you're playing on OGS",
                label(server)
            ),
        );
    }
    let same = {
        let mut sessions = state.sessions.lock().expect("sessions lock");
        let Some(s) = sessions.map.get_mut(&server) else {
            return;
        };
        s.searching = false;
        let same = s.game.as_ref().is_some_and(|old| old.id == g.id);
        if !same {
            s.game = Some(SessionGame {
                id: g.id,
                my_color: g.you,
                phase: Phase::Play,
                dead: Vec::new(),
            });
        } else if let Some(old) = s.game.as_mut() {
            old.my_color = g.you;
        }
        let was = sessions.active.replace(server);
        same && was == Some(server)
    };
    if !same {
        // Another server's game (or OGS's) no longer owns the board.
        live::close_game(&state);
    }
    {
        let mut game = state.game.lock().expect("game lock");
        let (date, time) = if same {
            (game.info.date.clone(), game.info.time.clone())
        } else {
            let now = chrono::Local::now();
            (
                Some(now.format("%Y-%m-%d").to_string()),
                Some(now.format("%H:%M").to_string()),
            )
        };
        let mut record = Game::from_info(GameInfo {
            size: g.size,
            komi: g.komi,
            black: Some(g.black.clone()),
            white: Some(g.white.clone()),
            black_rank: g.black_rank.clone(),
            white_rank: g.white_rank.clone(),
            date,
            time,
            server: Some(label(server).to_string()),
            setup_black: g.handicap.clone(),
            ..Default::default()
        });
        for (color, mv) in &g.moves {
            record.play_record(*color, *mv);
        }
        record.info.result = if same { game.info.result.clone() } else { None };
        *game = record;
    }
    push_view(app);
    push_status(app);
    if !same {
        let opponent = match g.you {
            Some(Color::Black) => &g.white,
            _ => &g.black,
        };
        notify(app, format!("Playing {opponent} on {}", label(server)));
    }
}

fn announce(app: &AppHandle, result: &str, mine: Option<Color>) {
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

// ---------- playing ----------

pub fn find_match(app: &AppHandle, server: Server, size: u8, speed: Speed) -> Result<(), String> {
    let state = app.state::<AppState>();
    if live::in_game(&state) || in_game(&state) {
        return Err("You're already in a game".into());
    }
    let client = {
        let sessions = state.sessions.lock().expect("sessions lock");
        let s = sessions.map.get(&server).filter(|s| s.connected);
        s.and_then(|s| s.client.clone())
            .ok_or_else(|| format!("Sign in to {} to play there", label(server)))?
    };
    client.find_match(size, speed)?;
    with_session(app, server, |s| s.searching = true);
    push_status(app);
    Ok(())
}

pub fn cancel_match(state: &AppState) {
    let mut sessions = state.sessions.lock().expect("sessions lock");
    for s in sessions.map.values_mut() {
        if s.searching {
            s.searching = false;
            if let Some(c) = &s.client {
                c.cancel_match();
            }
        }
    }
}

/// A click on the board during a KGS or Pandanet game: a move, or while
/// scoring, marking the group under it. `None` means it isn't for a server.
pub fn click(state: &AppState, point: Point) -> Option<Result<(), String>> {
    let (client, g) = active(state)?;
    match g.phase {
        Phase::Finished => None,
        Phase::Scoring if !client.marks_dead_stones() => Some(Err(
            "This server counts the game itself. Use Accept score".into(),
        )),
        Phase::Scoring => {
            let game = state.game.lock().expect("game lock");
            if game.board().get(point).is_none() {
                return Some(Err("Click a stone to mark it dead or alive".into()));
            }
            drop(game);
            client.toggle_dead(g.id, point, !g.dead.contains(&point));
            Some(Ok(()))
        }
        Phase::Play => {
            let mut game = state.game.lock().expect("game lock");
            let total = game.moves().len();
            game.go_to(total);
            if Some(game.to_move()) != g.my_color {
                return Some(Err("It's your opponent's turn".into()));
            }
            // Check legality here for an instant answer; the server echoes the move.
            let mut trial = game.clone();
            if let Err(e) = trial.play(point) {
                return Some(Err(e.to_string()));
            }
            drop(game);
            client.play(g.id, Move::Play { point });
            Some(Ok(()))
        }
    }
}

pub fn pass(state: &AppState) -> Option<Result<(), String>> {
    let (client, g) = active(state)?;
    if g.phase == Phase::Finished {
        return None;
    }
    let game = state.game.lock().expect("game lock");
    if g.phase != Phase::Play || Some(game.to_move()) != g.my_color {
        return Some(Err("It's your opponent's turn".into()));
    }
    drop(game);
    client.play(g.id, Move::Pass);
    Some(Ok(()))
}

pub fn resign(state: &AppState) -> Option<()> {
    let (client, g) = active(state)?;
    (g.phase != Phase::Finished).then(|| client.resign(g.id))
}

pub fn accept_score(state: &AppState) -> Option<()> {
    let (client, g) = active(state)?;
    (g.phase == Phase::Scoring).then(|| client.accept_score(g.id))
}
