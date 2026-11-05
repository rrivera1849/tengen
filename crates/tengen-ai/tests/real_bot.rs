//! Judging tried moves and playing as a person, against a real KataGo with
//! its human-style network. Skipped unless all three are set:
//! `TENGEN_KATAGO=… TENGEN_KATAGO_MODEL=… TENGEN_KATAGO_HUMAN_MODEL=… cargo test -p tengen-ai --test real_bot`

use std::path::PathBuf;

use tengen_ai::bot::{self, Choice, Mood};
use tengen_ai::judge::{self, Verdict};
use tengen_ai::katago::{from_gtp, Engine, Query};
use tengen_ai::runtime;
use tengen_core::{Color, Game, Move, Point};

async fn engine_or_skip() -> Option<Engine> {
    for k in [
        "TENGEN_KATAGO",
        "TENGEN_KATAGO_MODEL",
        "TENGEN_KATAGO_HUMAN_MODEL",
    ] {
        if std::env::var_os(k).is_none() {
            eprintln!("skipping: set {k}");
            return None;
        }
    }
    let dir: PathBuf = std::env::temp_dir().join("tengen bot test").join("katago");
    std::fs::create_dir_all(&dir).unwrap();
    let rt = runtime::locate(&dir).expect("runtime from environment");
    assert!(rt.human_model.is_some(), "the human model is found");
    // With only the human network at hand, it can stand in as the main one
    // too, given a profile to imitate: a strong one, for judging.
    if std::env::var_os("TENGEN_KATAGO_MODEL") == std::env::var_os("TENGEN_KATAGO_HUMAN_MODEL") {
        let mut cfg = std::fs::read_to_string(&rt.config).unwrap();
        cfg.push_str("humanSLProfile = proyear_2023\n");
        std::fs::write(&rt.config, cfg).unwrap();
    }
    Some(
        Engine::start(&rt.binary, &rt.model, rt.human_model.as_deref(), &rt.config)
            .await
            .expect("KataGo starts"),
    )
}

fn opening() -> Game {
    let mut g = Game::new(9, 7.0);
    for (x, y) in [(2, 2), (6, 6), (6, 2), (2, 6)] {
        g.play(Point::new(x, y)).unwrap();
    }
    g
}

#[tokio::test(flavor = "multi_thread")]
async fn judges_and_plays_with_real_katago() {
    let Some(engine) = engine_or_skip().await else {
        return;
    };

    // ---- judging ----
    let game = opening();
    let turn = game.moves().len();
    let q = Query::for_prefix(&game, turn, 200);
    let r = engine.analyze(q, |_, _| {}).await.unwrap();
    let best = from_gtp(&r[0].best().unwrap().mv, 9).expect("a move, not a pass");
    let j = judge::judge(&engine, &game, turn, best, 3.0).await.unwrap();
    assert_eq!(
        j.verdict,
        Verdict::Best,
        "KataGo's own choice is best: {j:?}"
    );

    // The corner point gives away the center: clearly worse.
    let j = judge::judge(&engine, &game, turn, Point::new(0, 0), 3.0)
        .await
        .unwrap();
    eprintln!("1-1 point: {j:?}");
    assert!(j.loss > 1.0, "the 1-1 point loses points: {j:?}");
    assert_ne!(j.verdict, Verdict::Best);

    assert!(
        judge::judge(&engine, &game, turn, Point::new(2, 2), 3.0)
            .await
            .is_err(),
        "an occupied point is refused"
    );

    // ---- the human policy ----
    let empty = Game::new(9, 7.0);
    let look = bot::look(&engine, &empty, "5k").await.unwrap();
    let policy = look.human_policy.as_ref().unwrap();
    assert_eq!(policy.len(), 82);
    let sum: f64 = policy.iter().filter(|p| **p > 0.0).sum();
    assert!((sum - 1.0).abs() < 0.01, "the policy sums to 1: {sum}");
    assert!(bot::look(&engine, &empty, "42k").await.is_err());

    // ---- a short game, the bot playing both sides at different ranks ----
    let mut g = Game::new(9, 7.0);
    let mut moods = [Mood::default(), Mood::default()];
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    let mut passes = 0;
    for n in 0..24 {
        let color = g.to_move();
        let (rank, mood) = if color == Color::Black {
            ("15k", &mut moods[0])
        } else {
            ("3d", &mut moods[1])
        };
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let roll = (x >> 11) as f64 / (1u64 << 53) as f64;
        match bot::choose(&engine, &g, rank, color, mood, roll)
            .await
            .unwrap()
        {
            Choice::Play(p) => {
                g.play(p)
                    .unwrap_or_else(|e| panic!("move {n} at {p:?} is illegal: {e}"));
                passes = 0;
            }
            Choice::Pass => {
                g.pass();
                passes += 1;
                if passes == 2 {
                    break;
                }
            }
            Choice::Resign => break,
        }
    }
    let stones = g
        .moves()
        .iter()
        .filter(|(_, m)| matches!(m, Move::Play { .. }))
        .count();
    eprintln!("bot game: {} moves, {stones} stones", g.moves().len());
    assert!(stones >= 10, "the bots played real moves");
}
