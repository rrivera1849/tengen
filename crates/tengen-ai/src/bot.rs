//! Playing against KataGo as a person of a chosen rank would: each move is
//! drawn from KataGo's human-style network for that rank, so it makes the
//! kind of moves (and mistakes) players of that strength make.

use serde_json::json;
use tengen_core::{Color, Game, Move, Point};

use crate::katago::{Engine, Query, TurnResult};
use crate::AiError;

/// The strengths offered, weakest first, as KataGo names them (`rank_5k`).
pub const RANKS: [&str; 9] = ["20k", "15k", "10k", "7k", "5k", "3k", "1k", "3d", "6d"];

/// Visits for the main network's look at the position: only used to decide
/// on resigning, so a light search is enough.
const VISITS: u32 = 40;
/// Moves a person of that rank would rarely play are left out of the draw.
const FLOOR: f64 = 0.01;
/// Below this chance of winning, for several moves running, the bot resigns.
const RESIGN_BELOW: f64 = 0.03;
const RESIGN_AFTER: usize = 3;

/// KataGo's profile name for a rank, if it's one Tengen offers.
pub fn profile(rank: &str) -> Option<String> {
    RANKS.contains(&rank).then(|| format!("rank_{rank}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Play(Point),
    Pass,
    Resign,
}

/// The bot's state across a game: how many of its moves in a row it was lost.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mood {
    hopeless: usize,
}

/// Picks a move from the human policy (row by row, then pass; negative is
/// illegal). `roll` is a random number in [0, 1).
///
/// Passing: when the opponent has just passed, the bot passes too if a
/// person would think about it (a fair share of the policy on pass) or no
/// move is clearly better. Otherwise it passes only when pass is a person's
/// most likely move.
pub fn draw(policy: &[f64], size: u8, opponent_passed: bool, roll: f64) -> Choice {
    let n = size as usize * size as usize;
    if policy.len() != n + 1 {
        return Choice::Pass;
    }
    let pass = policy[n].max(0.0);
    let best_point = policy[..n].iter().copied().fold(0.0_f64, f64::max);
    if pass >= best_point || (opponent_passed && pass >= 0.15) {
        return Choice::Pass;
    }
    // Leave out moves almost nobody would play, then draw in proportion.
    let floor = (best_point * FLOOR).max(1e-6);
    let weights: Vec<(usize, f64)> = policy[..n]
        .iter()
        .enumerate()
        .filter(|(_, &p)| p >= floor)
        .map(|(i, &p)| (i, p))
        .collect();
    let total: f64 = weights.iter().map(|(_, p)| p).sum();
    if weights.is_empty() || total <= 0.0 {
        return Choice::Pass;
    }
    let mut target = roll.clamp(0.0, 0.999_999) * total;
    for &(i, p) in &weights {
        if target < p {
            return Choice::Play(point(i, size));
        }
        target -= p;
    }
    Choice::Play(point(weights[weights.len() - 1].0, size))
}

fn point(index: usize, size: u8) -> Point {
    let s = size as usize;
    Point::new((index % s) as u8, (index / s) as u8)
}

/// True once the bot has been nearly lost for several moves in a row, and
/// the game is far enough along that resigning is reasonable.
pub fn should_resign(mood: &mut Mood, win_chance: f64, moves: usize, size: u8) -> bool {
    if win_chance < RESIGN_BELOW {
        mood.hopeless += 1;
    } else {
        mood.hopeless = 0;
    }
    let far_enough = moves >= (size as usize * size as usize) / 3;
    far_enough && mood.hopeless >= RESIGN_AFTER
}

/// The bot's chance of winning, from KataGo's Black-side win rate.
pub fn win_chance(r: &TurnResult, bot: Color) -> f64 {
    match bot {
        Color::Black => r.root_info.winrate,
        Color::White => 1.0 - r.root_info.winrate,
    }
}

/// Asks KataGo for the position at the end of `game`, with the human policy
/// for `rank`. Fails if no human-style network is loaded.
pub async fn look(engine: &Engine, game: &Game, rank: &str) -> Result<TurnResult, AiError> {
    let profile = profile(rank).ok_or_else(|| AiError::Other(format!("unknown rank {rank}")))?;
    let mut q = Query::for_prefix(game, game.moves().len(), VISITS);
    q.include_policy = true;
    q.override_settings = Some(json!({ "humanSLProfile": profile }));
    let mut results = engine.analyze(q, |_, _| {}).await?;
    if results.is_empty() {
        return Err(AiError::Engine("no answer came back".into()));
    }
    let r = results.remove(0);
    if r.human_policy.is_none() {
        return Err(AiError::Other(
            "KataGo's human-style network isn't loaded".into(),
        ));
    }
    Ok(r)
}

/// The bot's move at the end of `game`: a draw from the human policy,
/// or resigning when it has been lost for a while.
pub async fn choose(
    engine: &Engine,
    game: &Game,
    rank: &str,
    bot: Color,
    mood: &mut Mood,
    roll: f64,
) -> Result<Choice, AiError> {
    let r = look(engine, game, rank).await?;
    let moves = game.moves().len();
    if should_resign(mood, win_chance(&r, bot), moves, game.info.size) {
        return Ok(Choice::Resign);
    }
    let opponent_passed = matches!(game.moves().last(), Some((_, Move::Pass)));
    let policy = r.human_policy.as_deref().unwrap_or_default();
    let mut choice = draw(policy, game.info.size, opponent_passed, roll);
    // A draw can still land on an illegal point if KataGo and Tengen ever
    // disagree (superko, say); fall back to the person's likeliest legal move.
    if let Choice::Play(p) = choice {
        let mut trial = game.clone();
        trial.go_to(moves);
        if trial.play(p).is_err() {
            choice = likeliest_legal(policy, game);
        }
    }
    Ok(choice)
}

fn likeliest_legal(policy: &[f64], game: &Game) -> Choice {
    let size = game.info.size;
    let n = size as usize * size as usize;
    let mut order: Vec<usize> = (0..n.min(policy.len()))
        .filter(|&i| policy[i] > 0.0)
        .collect();
    order.sort_by(|a, b| policy[*b].total_cmp(&policy[*a]));
    for i in order {
        let mut trial = game.clone();
        trial.go_to(game.moves().len());
        if trial.play(point(i, size)).is_ok() {
            return Choice::Play(point(i, size));
        }
    }
    Choice::Pass
}

/// A result like `B+3.5` from Black's score lead, rounded the way the komi
/// makes real scores fall (half points with a half-point komi).
pub fn result_from_lead(lead: f64, komi: f32) -> String {
    let half = komi.fract().abs() > 0.25;
    let mut margin = if half {
        (lead.abs() - 0.5).round() + 0.5
    } else {
        lead.abs().round()
    };
    if margin == 0.0 {
        if half {
            margin = 0.5;
        } else {
            return "Draw".into();
        }
    }
    let n = if margin.fract() == 0.0 {
        format!("{margin:.0}")
    } else {
        format!("{margin:.1}")
    };
    format!("{}+{n}", if lead > 0.0 { "B" } else { "W" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(size: u8, picks: &[(u8, u8, f64)], pass: f64) -> Vec<f64> {
        let n = size as usize * size as usize;
        let mut p = vec![0.0; n + 1];
        for &(x, y, v) in picks {
            p[y as usize * size as usize + x as usize] = v;
        }
        p[n] = pass;
        p
    }

    #[test]
    fn ranks_map_to_katago_profiles() {
        assert_eq!(profile("5k").as_deref(), Some("rank_5k"));
        assert_eq!(profile("3d").as_deref(), Some("rank_3d"));
        assert_eq!(profile("99k"), None);
        assert_eq!(profile("rank_5k"), None, "only Tengen's own names");
    }

    #[test]
    fn draws_in_proportion_to_the_policy() {
        let p = policy(9, &[(2, 2, 0.6), (6, 6, 0.3), (4, 4, 0.1)], 0.0);
        assert_eq!(draw(&p, 9, false, 0.0), Choice::Play(Point::new(2, 2)));
        assert_eq!(draw(&p, 9, false, 0.59), Choice::Play(Point::new(2, 2)));
        // Points are taken in board order: C7 (0.6), E5 (0.1), G3 (0.3).
        assert_eq!(draw(&p, 9, false, 0.65), Choice::Play(Point::new(4, 4)));
        assert_eq!(draw(&p, 9, false, 0.95), Choice::Play(Point::new(6, 6)));
        assert_eq!(
            draw(&p, 9, false, 1.0),
            Choice::Play(Point::new(6, 6)),
            "1.0 stays in range"
        );
        // Over many draws, each move comes up about as often as its share.
        let mut counts = [0usize; 3];
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let roll = (x >> 11) as f64 / (1u64 << 53) as f64;
            match draw(&p, 9, false, roll) {
                Choice::Play(pt) if pt == Point::new(2, 2) => counts[0] += 1,
                Choice::Play(pt) if pt == Point::new(6, 6) => counts[1] += 1,
                _ => counts[2] += 1,
            }
        }
        assert!((5500..6500).contains(&counts[0]), "{counts:?}");
        assert!((2500..3500).contains(&counts[1]), "{counts:?}");
    }

    #[test]
    fn never_draws_illegal_or_rare_moves() {
        let mut p = policy(9, &[(2, 2, 0.99)], 0.0);
        p[0] = -1.0; // illegal
        p[1] = 0.000_5; // almost nobody plays it
        for i in 0..100 {
            assert_eq!(
                draw(&p, 9, false, i as f64 / 100.0),
                Choice::Play(Point::new(2, 2))
            );
        }
    }

    #[test]
    fn passes_like_a_person() {
        let likely_pass = policy(9, &[(2, 2, 0.2)], 0.7);
        assert_eq!(draw(&likely_pass, 9, false, 0.5), Choice::Pass);
        let some_pass = policy(9, &[(2, 2, 0.6)], 0.2);
        assert_eq!(
            draw(&some_pass, 9, false, 0.1),
            Choice::Play(Point::new(2, 2)),
            "keeps playing"
        );
        assert_eq!(
            draw(&some_pass, 9, true, 0.1),
            Choice::Pass,
            "answers a pass"
        );
        assert_eq!(
            draw(&[0.5; 3], 9, false, 0.1),
            Choice::Pass,
            "a malformed policy passes"
        );
    }

    #[test]
    fn resigns_only_when_lost_for_a_while_and_late_enough() {
        let mut m = Mood::default();
        assert!(!should_resign(&mut m, 0.01, 10, 9), "too early");
        assert!(!should_resign(&mut m, 0.01, 40, 9));
        assert!(
            should_resign(&mut m, 0.01, 41, 9),
            "third hopeless move in a row"
        );
        let mut m = Mood::default();
        assert!(!should_resign(&mut m, 0.01, 40, 9));
        assert!(!should_resign(&mut m, 0.2, 41, 9), "hope resets the count");
        assert!(!should_resign(&mut m, 0.01, 42, 9));
    }

    #[test]
    fn results_round_like_real_scores() {
        assert_eq!(result_from_lead(3.2, 6.5), "B+3.5");
        assert_eq!(result_from_lead(-0.1, 6.5), "W+0.5");
        assert_eq!(result_from_lead(-12.9, 7.5), "W+12.5");
        assert_eq!(result_from_lead(4.4, 7.0), "B+4");
        assert_eq!(result_from_lead(0.3, 7.0), "Draw");
    }

    #[test]
    fn likeliest_legal_skips_occupied_points() {
        let mut g = Game::new(9, 7.0);
        g.play(Point::new(2, 2)).unwrap();
        let p = policy(9, &[(2, 2, 0.9), (6, 6, 0.1)], 0.0);
        assert_eq!(likeliest_legal(&p, &g), Choice::Play(Point::new(6, 6)));
    }
}
