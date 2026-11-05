//! Judging a move tried in the review's "Try again": KataGo scores it against
//! its own choice, so a move nearly as good counts even when it isn't the one.

use serde::Serialize;
use tengen_core::{Color, Game, Move, Point};

use crate::katago::{from_gtp, Engine, Query, TurnResult};
use crate::AiError;

/// Visits for each side of the comparison: enough for a steady score.
const VISITS: u32 = 300;
/// A move losing at most this many points (against KataGo's best) is good.
pub const GOOD: f64 = 1.0;
/// Within this, it's as good as KataGo's own choice.
pub const BEST: f64 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// KataGo's move, or one just as good.
    Best,
    /// Not KataGo's first choice, but within a point of it.
    Good,
    /// Clearly better than the move in the game, but still losing points.
    Closer,
    /// No better than the move in the game.
    Worse,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Judgement {
    pub verdict: Verdict,
    /// Points the tried move loses against KataGo's best, never negative.
    pub loss: f64,
}

/// What a move's loss means, given how much the game's move lost.
pub fn verdict(loss: f64, played_loss: f64) -> Verdict {
    if loss <= BEST {
        Verdict::Best
    } else if loss <= GOOD {
        Verdict::Good
    } else if loss < played_loss * 0.6 {
        Verdict::Closer
    } else {
        Verdict::Worse
    }
}

/// Points lost by the mover, from Black's score lead with best play before
/// the move and Black's score lead after it. Small negative noise counts as 0.
pub fn loss(mover: Color, lead_before: f64, lead_after: f64) -> f64 {
    let sign = if mover == Color::Black { 1.0 } else { -1.0 };
    (sign * (lead_before - lead_after)).max(0.0)
}

/// The position after the first `turn` moves of `game`, as its own game.
pub fn position(game: &Game, turn: usize) -> Game {
    let mut g = Game::from_info(game.info.clone());
    for (color, mv) in game.moves().iter().take(turn) {
        g.play_record(*color, *mv);
    }
    g
}

/// Scores `tried` at the position after `turn` moves. `played_loss` is how
/// much the game's move lost there. Illegal moves are an error.
pub async fn judge(
    engine: &Engine,
    game: &Game,
    turn: usize,
    tried: Point,
    played_loss: f64,
) -> Result<Judgement, AiError> {
    let before = position(game, turn);
    let mover = before.to_move();
    let mut after = before.clone();
    after
        .play(tried)
        .map_err(|e| AiError::Other(format!("That move isn't legal here: {e}")))?;

    // Both positions at once.
    let q_before = Query::for_prefix(&before, turn, VISITS);
    let q_after = Query::for_prefix(&after, turn + 1, VISITS);
    let (a, b) = tokio::join!(
        engine.analyze(q_before, |_, _| {}),
        engine.analyze(q_after, |_, _| {})
    );
    let a = first(a?)?;
    let b = first(b?)?;
    // If KataGo's own first choice is the tried move, it's best by definition.
    let size = game.info.size;
    if a.best().and_then(|m| from_gtp(&m.mv, size)) == Some(tried) {
        return Ok(Judgement {
            verdict: Verdict::Best,
            loss: 0.0,
        });
    }
    let l = loss(mover, a.root_info.score_lead, b.root_info.score_lead);
    Ok(Judgement {
        verdict: verdict(l, played_loss),
        loss: (l * 10.0).round() / 10.0,
    })
}

fn first(mut results: Vec<TurnResult>) -> Result<TurnResult, AiError> {
    if results.is_empty() {
        return Err(AiError::Engine("no answer came back".into()));
    }
    Ok(results.remove(0))
}

/// The move at `turn` in the record, if it was a stone.
pub fn played_at(game: &Game, turn: usize) -> Option<Point> {
    match game.moves().get(turn)? {
        (_, Move::Play { point }) => Some(*point),
        (_, Move::Pass) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_follow_the_loss() {
        assert_eq!(verdict(0.1, 3.0), Verdict::Best);
        assert_eq!(verdict(0.8, 3.0), Verdict::Good);
        assert_eq!(
            verdict(1.5, 4.0),
            Verdict::Closer,
            "much better than the game's 4"
        );
        assert_eq!(
            verdict(2.9, 4.0),
            Verdict::Worse,
            "barely better isn't closer"
        );
        assert_eq!(verdict(5.0, 3.0), Verdict::Worse);
        // A small mistake: anything past a point is no improvement worth praise.
        assert_eq!(verdict(1.2, 1.5), Verdict::Worse);
    }

    #[test]
    fn loss_is_from_the_movers_side() {
        assert_eq!(loss(Color::Black, 3.0, 1.0), 2.0, "Black's lead fell by 2");
        assert_eq!(
            loss(Color::White, -3.0, -1.0),
            2.0,
            "White's lead fell by 2"
        );
        assert_eq!(
            loss(Color::White, -3.0, -4.0),
            0.0,
            "a gain is noise: no loss"
        );
    }

    #[test]
    fn position_stops_at_the_turn() {
        let mut g = Game::new(9, 7.0);
        for (x, y) in [(2, 2), (6, 6), (4, 4)] {
            g.play(Point::new(x, y)).unwrap();
        }
        let p = position(&g, 2);
        assert_eq!(p.moves().len(), 2);
        assert_eq!(p.to_move(), Color::Black);
        assert!(p.board().get(Point::new(4, 4)).is_none());
        assert_eq!(played_at(&g, 2), Some(Point::new(4, 4)));
        assert_eq!(played_at(&g, 9), None);
    }

    #[test]
    fn prefix_queries_drop_later_moves() {
        let mut g = Game::new(9, 7.0);
        for (x, y) in [(2, 2), (6, 6), (4, 4)] {
            g.play(Point::new(x, y)).unwrap();
        }
        let q = Query::for_prefix(&g, 2, 50);
        assert_eq!(q.moves.len(), 2);
        assert_eq!(q.analyze_turns, vec![2]);
    }
}
