//! The score estimate: one KataGo look at the current position.

use serde::Serialize;
use tengen_core::{Board, Color, Game, Point};

use crate::katago::{Engine, Query, TurnResult};
use crate::AiError;

/// How sure KataGo must be before an intersection is shown as someone's.
const OWNED: f64 = 0.6;
const VISITS: u32 = 300;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    /// Points Black leads by; negative when White leads.
    pub lead: f64,
    /// Black's chance of winning, 0 to 1.
    pub black_win_chance: f64,
    /// Empty intersections, and dead stones, that clearly belong to someone.
    pub territory: Vec<(u8, u8, Color)>,
    /// Stones KataGo considers dead.
    pub dead: Vec<(u8, u8)>,
    /// "Black leads by 4.5", ready to show.
    pub headline: String,
}

pub async fn estimate(engine: &Engine, game: &Game) -> Result<Estimate, AiError> {
    let query = Query::for_game(game, vec![game.cursor()], VISITS, true);
    let result = engine.analyze(query, |_, _| {}).await?;
    let r = result
        .first()
        .ok_or_else(|| AiError::Engine("no estimate came back".into()))?;
    Ok(from_result(r, game.board()))
}

pub fn from_result(r: &TurnResult, board: &Board) -> Estimate {
    let size = board.size();
    let mut territory = Vec::new();
    let mut dead = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let p = Point::new(x, y);
            let Some(own) = r.ownership_at(p, size) else {
                continue;
            };
            let owner = if own >= OWNED {
                Some(Color::Black)
            } else if own <= -OWNED {
                Some(Color::White)
            } else {
                None
            };
            match (board.get(p), owner) {
                (None, Some(c)) => territory.push((x, y, c)),
                (Some(stone), Some(c)) if stone != c => {
                    dead.push((x, y));
                    territory.push((x, y, c));
                }
                _ => {}
            }
        }
    }
    let lead = r.root_info.score_lead;
    Estimate {
        lead,
        black_win_chance: r.root_info.winrate,
        territory,
        dead,
        headline: headline(lead),
    }
}

/// Rounds to the nearest half point, as scores are counted.
pub fn headline(lead: f64) -> String {
    let half = (lead.abs() * 2.0).round() / 2.0;
    if half < 0.5 {
        return "Even".into();
    }
    let n = if half.fract() == 0.0 {
        format!("{half:.0}")
    } else {
        format!("{half:.1}")
    };
    format!(
        "{} leads by {n}",
        if lead > 0.0 { "Black" } else { "White" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::katago::RootInfo;

    #[test]
    fn headlines_round_to_half_points() {
        assert_eq!(headline(4.3), "Black leads by 4.5");
        assert_eq!(headline(-12.04), "White leads by 12");
        assert_eq!(headline(0.2), "Even");
    }

    #[test]
    fn reads_territory_and_dead_stones() {
        let mut b = Board::new(2);
        b.set(Point::new(0, 0), Some(Color::White));
        let r = TurnResult {
            turn_number: 0,
            root_info: RootInfo {
                winrate: 0.9,
                score_lead: 3.0,
                visits: 1,
            },
            move_infos: vec![],
            ownership: Some(vec![0.95, 0.9, 0.2, -0.8]),
        };
        let e = from_result(&r, &b);
        assert_eq!(e.dead, vec![(0, 0)], "the white stone sits in Black's area");
        assert!(e.territory.contains(&(1, 0, Color::Black)));
        assert!(e.territory.contains(&(1, 1, Color::White)));
        assert!(
            !e.territory.iter().any(|t| (t.0, t.1) == (0, 1)),
            "0.2 is unsettled"
        );
    }
}
