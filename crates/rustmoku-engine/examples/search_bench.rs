use std::time::Instant;

use rustmoku_core::{Move, Position};
use rustmoku_engine::{AlphaBetaEngine, SearchEngine, SearchLimits};

const OPENING: &[(usize, usize)] = &[(7, 7), (6, 7), (8, 8), (7, 8)];

const BALANCED_MIDGAME: &[(usize, usize)] = &[
    (7, 7),
    (7, 8),
    (8, 8),
    (6, 6),
    (8, 7),
    (6, 8),
    (9, 6),
    (5, 9),
    (9, 8),
    (5, 7),
    (6, 9),
    (8, 6),
];

const TACTICAL_ATTACK: &[(usize, usize)] = &[
    (7, 3),
    (0, 0),
    (7, 4),
    (0, 2),
    (7, 5),
    (1, 0),
    (7, 6),
    (1, 2),
];

const FORCED_DEFENSE: &[(usize, usize)] = &[
    (7, 2),
    (7, 3),
    (0, 0),
    (7, 4),
    (0, 2),
    (7, 5),
    (1, 0),
    (7, 6),
];

const TRANSPOSITION_RICH: &[(usize, usize)] = &[
    (7, 7),
    (7, 8),
    (8, 7),
    (6, 7),
    (8, 8),
    (6, 8),
    (9, 6),
    (5, 9),
    (9, 9),
    (5, 6),
];

struct Fixture {
    name: &'static str,
    moves: &'static [(usize, usize)],
    depth: u8,
}

fn main() {
    let fixtures = [
        Fixture {
            name: "opening",
            moves: OPENING,
            depth: 4,
        },
        Fixture {
            name: "balanced_midgame",
            moves: BALANCED_MIDGAME,
            depth: 4,
        },
        Fixture {
            name: "tactical_attack",
            moves: TACTICAL_ATTACK,
            depth: 4,
        },
        Fixture {
            name: "forced_defense",
            moves: FORCED_DEFENSE,
            depth: 4,
        },
        Fixture {
            name: "transposition_rich",
            moves: TRANSPOSITION_RICH,
            depth: 4,
        },
    ];

    println!(
        "fixture,requested_depth,completed_depth,seldepth,best_move,score,nodes,tt_hits,tt_cutoffs,elapsed_ms,nps"
    );
    for fixture in fixtures {
        let position = build_position(fixture.moves);
        let mut engine = AlphaBetaEngine::default();
        let started = Instant::now();
        let result = engine.search(&position, SearchLimits::new(fixture.depth));
        let elapsed = started.elapsed();
        let nps = if elapsed.is_zero() {
            0
        } else {
            (result.statistics.nodes as f64 / elapsed.as_secs_f64()).round() as u64
        };
        println!(
            "{},{},{},{},{},{},{},{},{},{:.3},{}",
            fixture.name,
            fixture.depth,
            result.completed_depth,
            result.seldepth,
            format_move(result.best_move),
            result.score,
            result.statistics.nodes,
            result.statistics.tt_hits,
            result.statistics.tt_cutoffs,
            elapsed.as_secs_f64() * 1_000.0,
            nps,
        );
    }
}

fn build_position(moves: &[(usize, usize)]) -> Position {
    let mut position = Position::default();
    for &(row, column) in moves {
        let at = Move::from_row_col(row, column).expect("benchmark coordinates must be valid");
        position
            .make_move(at)
            .expect("benchmark move sequence must remain legal");
    }
    position
}

fn format_move(at: Option<Move>) -> String {
    match at {
        Some(at) => format!("{}({},{})", at.index(), at.row(), at.column()),
        None => String::from("none"),
    }
}
