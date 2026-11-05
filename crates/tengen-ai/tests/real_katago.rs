//! Runs against a real KataGo. Skipped unless both are set:
//! `TENGEN_KATAGO=/path/to/katago TENGEN_KATAGO_MODEL=/path/to/net.bin.gz cargo test -p tengen-ai --test real_katago`

use std::path::PathBuf;

use tengen_ai::katago::{Engine, Query};
use tengen_ai::{analysis, estimate, review, runtime};
use tengen_core::{Color, Game, Point};

fn runtime_or_skip() -> Option<runtime::Runtime> {
    if std::env::var_os("TENGEN_KATAGO").is_none()
        || std::env::var_os("TENGEN_KATAGO_MODEL").is_none()
    {
        eprintln!("skipping: set TENGEN_KATAGO and TENGEN_KATAGO_MODEL");
        return None;
    }
    // A folder with a space in it, like ~/Library/Application Support.
    let dir: PathBuf = std::env::temp_dir()
        .join("tengen test runtime")
        .join("katago");
    std::fs::create_dir_all(&dir).unwrap();
    Some(runtime::locate(&dir).expect("runtime from environment"))
}

/// Black walls off the top of a 9×9 board; White sits in the bottom right.
fn sample_game() -> Game {
    let mut g = Game::new(9, 7.0);
    for (x, y) in [
        (2, 2),
        (6, 6),
        (6, 2),
        (2, 6),
        (4, 2),
        (6, 4),
        (4, 3),
        (5, 5),
    ] {
        g.play(Point::new(x, y)).unwrap();
    }
    g
}

#[tokio::test(flavor = "multi_thread")]
async fn analyses_estimates_and_reviews_with_real_katago() {
    let Some(rt) = runtime_or_skip() else { return };
    let engine = Engine::start(&rt.binary, &rt.model, rt.human_model.as_deref(), &rt.config)
        .await
        .expect("KataGo starts");
    let game = sample_game();

    // Ownership sign: Black's stone at C7 should be strongly Black's (+).
    let mut q = Query::for_game(&game, vec![game.moves().len()], 200, true);
    q.analyze_turns = vec![game.moves().len()];
    let r = engine.analyze(q, |_, _| {}).await.expect("analysis");
    let own = r[0].ownership_at(Point::new(2, 2), 9).expect("ownership");
    assert!(own > 0.5, "Black's stone should read as Black's: got {own}");
    let own_w = r[0].ownership_at(Point::new(6, 6), 9).expect("ownership");
    assert!(
        own_w < -0.5,
        "White's stone should read as White's: got {own_w}"
    );

    let e = estimate::estimate(&engine, &game).await.expect("estimate");
    assert!(!e.territory.is_empty());
    eprintln!(
        "estimate: {} ({} owned points)",
        e.headline,
        e.territory.len()
    );

    // A quick pass over every position, in order.
    let mut seen = 0;
    let quick = analysis::quick_pass(&engine, &game, |done, _| seen = done)
        .await
        .expect("quick pass");
    assert_eq!(quick.len(), game.moves().len() + 1);
    assert_eq!(seen, quick.len());
    assert!(quick
        .windows(2)
        .all(|w| w[0].turn_number < w[1].turn_number));

    // Every move a candidate, to exercise the deep pass and fact sheets.
    let all: Vec<usize> = (0..game.moves().len()).collect();
    let deep = analysis::deep_pass(&engine, &game, review::deep_turns(&all), |_, _| {})
        .await
        .expect("deep pass");
    let mut a = analysis::GameAnalysis {
        quick,
        ..Default::default()
    };
    for r in deep {
        a.deep.insert(r.turn_number, r);
    }
    let lessons = review::episodes(&game, &a, Color::Black, &all);
    eprintln!("{} lessons", lessons.len());
    for l in &lessons {
        assert!(!l.facts.board.is_empty());
        assert!(l.facts.coordinates().contains(&l.facts.played.coord));
        eprintln!(
            "  move {} {:?} {:?}: {} points",
            l.index + 1,
            l.side,
            l.kind,
            l.points_lost
        );
    }
    if let Some(l) = lessons.first() {
        eprintln!(
            "fact sheet:\n{}",
            serde_json::to_string_pretty(&l.facts).unwrap()
        );
    }
}
