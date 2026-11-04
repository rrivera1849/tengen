//! The two passes over a game: a quick look at every position to find
//! candidates, then a deep look at each candidate to confirm it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tengen_core::{sgf, Game};

use crate::katago::{Engine, Query, TurnResult};
use crate::AiError;

pub const QUICK_VISITS: u32 = 100;
pub const DEEP_VISITS: u32 = 2000;

/// Everything KataGo said about one game, cached on disk by [`fingerprint`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameAnalysis {
    /// One result per position, turn 0 (before any move) to the last.
    pub quick: Vec<TurnResult>,
    /// Deep results with ownership, by turn.
    pub deep: BTreeMap<usize, TurnResult>,
}

impl GameAnalysis {
    /// Black's score lead at every position, for the score graph.
    pub fn leads(&self) -> Vec<f64> {
        self.quick.iter().map(|r| self.best_known(r.turn_number).root_info.score_lead).collect()
    }

    /// The deep result for a turn when there is one, else the quick one.
    pub fn best_known(&self, turn: usize) -> &TurnResult {
        self.deep.get(&turn).unwrap_or(&self.quick[turn])
    }
}

/// Identifies a game record, so its analysis can be reused. Comments don't
/// count, so writing a review into the game keeps its fingerprint.
pub fn fingerprint(game: &Game) -> String {
    let mut bare = game.clone();
    bare.comments.clear();
    let mut h = Sha256::new();
    h.update(sgf::write(&bare).as_bytes());
    h.update(format!("{QUICK_VISITS}/{DEEP_VISITS}").as_bytes());
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// Every position at a low visit count. `progress(done, total)` as turns finish.
pub async fn quick_pass(engine: &Engine, game: &Game, progress: impl FnMut(usize, usize)) -> Result<Vec<TurnResult>, AiError> {
    let turns: Vec<usize> = (0..=game.moves().len()).collect();
    engine.analyze(Query::for_game(game, turns, QUICK_VISITS, false), progress).await
}

/// The given positions at a high visit count, with ownership.
pub async fn deep_pass(
    engine: &Engine,
    game: &Game,
    turns: Vec<usize>,
    progress: impl FnMut(usize, usize),
) -> Result<Vec<TurnResult>, AiError> {
    if turns.is_empty() {
        return Ok(Vec::new());
    }
    engine.analyze(Query::for_game(game, turns, DEEP_VISITS, true), progress).await
}
