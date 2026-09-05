use std::{env, error::Error, time::Instant};

use rustmoku_core::{Move, Position};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, EngineConfig, Evaluator, PatternEvaluator, SearchEngine,
    SearchLimits,
};

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

fn main() -> Result<(), Box<dyn Error>> {
    let mut depth = 4;
    let mut classical = false;
    let mut memory_mib = 64;
    let mut repeats = 5;
    let mut fixture_filter = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--suite" => {
                depth = match args.next().as_deref() {
                    Some("quick") => 4,
                    Some("deep") => 6,
                    _ => return Err("--suite requires quick or deep".into()),
                }
            }
            "--evaluator" => {
                classical = match args.next().as_deref() {
                    Some("pattern") => false,
                    Some("classical") => true,
                    _ => return Err("--evaluator requires pattern or classical".into()),
                }
            }
            "--depth" => depth = args.next().ok_or("missing depth")?.parse()?,
            "--tt-mib" => memory_mib = args.next().ok_or("missing MiB")?.parse()?,
            "--repeats" => repeats = args.next().ok_or("missing repeats")?.parse::<usize>()?,
            "--fixture" => fixture_filter = Some(args.next().ok_or("missing fixture")?),
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if repeats == 0 {
        return Err("repeats must be positive".into());
    }
    let fixtures = [
        Fixture {
            name: "opening",
            moves: OPENING,
            depth,
        },
        Fixture {
            name: "balanced_midgame",
            moves: BALANCED_MIDGAME,
            depth,
        },
        Fixture {
            name: "tactical_attack",
            moves: TACTICAL_ATTACK,
            depth,
        },
        Fixture {
            name: "forced_defense",
            moves: FORCED_DEFENSE,
            depth,
        },
        Fixture {
            name: "transposition_rich",
            moves: TRANSPOSITION_RICH,
            depth,
        },
    ];

    if fixture_filter
        .as_ref()
        .is_some_and(|name| !fixtures.iter().any(|f| f.name == name))
    {
        return Err("unknown fixture".into());
    }
    println!(
        "fixture,evaluator,tt_mib,repeats,requested_depth,completed_depth,seldepth,best_index,score,nodes,qnodes,pvs_researches,lmr_reductions,lmr_researches,aspiration_fail_low,aspiration_fail_high,static_evaluations,tt_probes,tt_hits,tt_cutoffs,tt_stores,tt_replacements,capacity_bytes,buckets,entries,hashfull_per_mille,median_ms,nps"
    );
    for fixture in fixtures {
        if fixture_filter
            .as_ref()
            .is_some_and(|name| name != fixture.name)
        {
            continue;
        }
        if classical {
            benchmark(
                &fixture,
                ClassicalEvaluator,
                "classical",
                memory_mib,
                repeats,
            );
        } else {
            benchmark(&fixture, PatternEvaluator, "pattern", memory_mib, repeats);
        }
    }
    Ok(())
}

fn benchmark<E: Evaluator>(
    fixture: &Fixture,
    evaluator: E,
    name: &str,
    memory_mib: usize,
    repeats: usize,
) {
    let position = build_position(fixture.moves);
    let mut engine = AlphaBetaEngine::with_config(evaluator, EngineConfig::new(memory_mib));
    let limits = SearchLimits::new(fixture.depth);
    let reference = engine.search(&position, limits); // Untimed warm-up, cold TT below.
    let mut times = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        engine.clear_transposition_table(); // Exclude memory clearing from search time.
        let started = Instant::now();
        let result = engine.search(&position, limits);
        times.push(started.elapsed());
        assert_eq!(
            result, reference,
            "cold runs must reproduce semantic results and statistics"
        );
    }
    times.sort_unstable();
    let elapsed = times[times.len() / 2]; // Upper median for even sample counts.
    let result = reference;
    let stats = result.statistics;
    let tt = engine.transposition_table_statistics(); // Sampling outside timed region.
    let nps = stats.nodes as f64 / elapsed.as_secs_f64();
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.0}",
        fixture.name,
        name,
        memory_mib,
        repeats,
        fixture.depth,
        result.completed_depth,
        result.seldepth,
        result
            .best_move
            .map_or(String::from("none"), |at| at.index().to_string()),
        result.score,
        stats.nodes,
        stats.qnodes,
        stats.pvs_researches,
        stats.lmr_reductions,
        stats.lmr_researches,
        stats.aspiration_fail_low,
        stats.aspiration_fail_high,
        stats.static_evaluations,
        stats.tt_probes,
        stats.tt_hits,
        stats.tt_cutoffs,
        stats.tt_stores,
        stats.tt_replacements,
        tt.capacity_bytes,
        tt.bucket_count,
        tt.entry_count,
        tt.hashfull_per_mille,
        elapsed.as_secs_f64() * 1000.0,
        nps
    );
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
