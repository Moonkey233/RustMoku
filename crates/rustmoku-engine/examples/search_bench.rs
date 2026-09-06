use std::{
    env,
    error::Error,
    time::{Duration, Instant},
};

use rustmoku_core::{Move, Position};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, EngineConfig, Evaluator, PatternEvaluator, SearchEngine,
    SearchLimits, SearchResult, TranspositionTableStatistics,
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

const VCF_WIN: &[(usize, usize)] = &[
    (7, 3),
    (7, 2),
    (7, 4),
    (0, 0),
    (7, 5),
    (0, 2),
    (4, 6),
    (0, 4),
    (5, 6),
    (0, 6),
];
const NON_VCF_TACTICAL: &[(usize, usize)] = &[(7, 3), (7, 2), (7, 4), (0, 0), (7, 5), (0, 2)];

const VCT_WIN: &[(usize, usize)] = &[
    (7, 5),
    (0, 0),
    (7, 6),
    (0, 14),
    (5, 7),
    (14, 0),
    (6, 7),
    (14, 14),
];
const NON_VCT_TACTICAL: &[(usize, usize)] = &[(7, 5), (0, 0), (7, 6), (14, 14)];

struct Fixture {
    name: &'static str,
    moves: &'static [(usize, usize)],
    depth: u8,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut depth = 4;
    let mut classical = false;
    let mut memory_mib = 64;
    let mut threads = 1;
    let mut repeats = 5;
    let mut fixture_filter = None;
    let mut vcf_plies = EngineConfig::DEFAULT_VCF_MAX_PLIES;
    let mut vcf_nodes = EngineConfig::DEFAULT_VCF_MAX_NODES;
    let mut vct = EngineConfig::default().tactical().vct;
    let mut vct_memory = EngineConfig::default().tactical().vct_table_memory_mib;
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
            "--threads" => threads = args.next().ok_or("missing threads")?.parse()?,
            "--repeats" => repeats = args.next().ok_or("missing repeats")?.parse::<usize>()?,
            "--vcf-plies" => vcf_plies = args.next().ok_or("missing VCF plies")?.parse()?,
            "--vcf-nodes" => vcf_nodes = args.next().ok_or("missing VCF nodes")?.parse()?,
            "--vct-plies" => vct.max_plies = args.next().ok_or("missing VCT plies")?.parse()?,
            "--vct-nodes" => vct.max_nodes = args.next().ok_or("missing VCT nodes")?.parse()?,
            "--vct-mib" => vct_memory = args.next().ok_or("missing VCT MiB")?.parse()?,
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

    let proof_fixtures = [
        Fixture {
            name: "vct_win",
            moves: VCT_WIN,
            depth,
        },
        Fixture {
            name: "non_vct_tactical",
            moves: NON_VCT_TACTICAL,
            depth,
        },
        Fixture {
            name: "vcf_win",
            moves: VCF_WIN,
            depth,
        },
        Fixture {
            name: "non_vcf_tactical",
            moves: NON_VCF_TACTICAL,
            depth,
        },
    ];
    let config = EngineConfig::new(memory_mib)
        .with_threads(threads)
        .with_vcf_limits(vcf_plies, vcf_nodes)
        .with_vct_limits(vct.max_plies, vct.max_nodes)
        .with_vct_table_memory(vct_memory);
    if fixture_filter.as_ref().is_some_and(|name| {
        !fixtures
            .iter()
            .chain(&proof_fixtures)
            .any(|f| f.name == name)
    }) {
        return Err("unknown fixture".into());
    }
    println!(
        "fixture,evaluator,tt_mib,threads,repeats,requested_depth,completed_depth,seldepth,best_index,score,nodes,qnodes,principal_nodes,helper_nodes,pvs_researches,lmr_reductions,lmr_researches,aspiration_fail_low,aspiration_fail_high,static_evaluations,tt_probes,tt_hits,tt_cutoffs,tt_stores,tt_replacements,vcf_nodes,vcf_cache_hits,vcf_probes,vcf_proven,vcf_budget_exhausted,vct_nodes,vct_cache_hits,vct_proven,vct_budget_exhausted,capacity_bytes,synchronization_bytes,allocated_bytes,buckets,entries,hashfull_per_mille,median_ms,nps,work_nodes,termination"
    );
    for fixture in fixtures.iter().chain(&proof_fixtures) {
        if fixture_filter.is_none() && proof_fixtures.iter().any(|f| f.name == fixture.name) {
            continue;
        }
        if fixture_filter
            .as_ref()
            .is_some_and(|name| name != fixture.name)
        {
            continue;
        }
        if classical {
            benchmark(fixture, ClassicalEvaluator, "classical", config, repeats);
        } else {
            benchmark(fixture, PatternEvaluator, "pattern", config, repeats);
        }
    }
    Ok(())
}

fn benchmark<E: Evaluator>(
    fixture: &Fixture,
    evaluator: E,
    name: &str,
    config: EngineConfig,
    repeats: usize,
) {
    let position = build_position(fixture.moves);
    let memory_mib = config.tt_memory_mib();
    let mut engine = AlphaBetaEngine::with_config(evaluator, config);
    let limits = SearchLimits::new(fixture.depth);
    let reference = engine.search(&position, limits); // Untimed warm-up, cold TT below.
    let mut samples = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        engine.clear_transposition_table(); // Exclude memory clearing from search time.
        let started = Instant::now();
        let result = engine.search(&position, limits);
        let elapsed = started.elapsed();
        if config.threads() == 1 {
            assert_eq!(
                result, reference,
                "cold runs must reproduce semantic results and statistics"
            );
        }
        samples.push(BenchmarkSample {
            elapsed,
            result,
            table: engine.transposition_table_statistics(), // Untimed sampling.
        });
    }
    let sample = median_sample(&mut samples);
    let elapsed = sample.elapsed;
    let result = &sample.result;
    let stats = result.statistics;
    let tt = sample.table;
    let nps = stats.nodes as f64 / elapsed.as_secs_f64();
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.0},{},{:?}",
        fixture.name,
        name,
        memory_mib,
        config.threads(),
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
        stats.principal_nodes,
        stats.helper_nodes,
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
        stats.vcf_nodes,
        stats.vcf_cache_hits,
        stats.vcf_probes,
        stats.vcf_proven,
        stats.vcf_budget_exhausted,
        stats.vct_nodes,
        stats.vct_cache_hits,
        stats.vct_proven,
        stats.vct_budget_exhausted,
        tt.capacity_bytes,
        tt.synchronization_bytes,
        tt.allocated_bytes,
        tt.bucket_count,
        tt.entry_count,
        tt.hashfull_per_mille,
        elapsed.as_secs_f64() * 1000.0,
        nps,
        stats.work_nodes,
        result.termination
    );
}

struct BenchmarkSample {
    elapsed: Duration,
    result: SearchResult,
    table: TranspositionTableStatistics,
}

fn median_sample(samples: &mut [BenchmarkSample]) -> &BenchmarkSample {
    // Keep scheduling-dependent work, score and occupancy attached to the run
    // whose time we report. Use the upper median for an even sample count.
    samples.sort_by_key(|sample| sample.elapsed);
    &samples[samples.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_keeps_the_matching_result_and_table_statistics() {
        let result = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(0))
            .search(&Position::default(), SearchLimits::new(0));
        for durations in [vec![30, 20, 10], vec![40, 20, 30, 10]] {
            let expected = if durations.len() == 3 { 20 } else { 30 };
            let mut samples: Vec<_> = durations
                .into_iter()
                .map(|millis| {
                    let mut result = result.clone();
                    result.statistics.nodes = millis * 100;
                    result.score = millis as i32;
                    BenchmarkSample {
                        elapsed: Duration::from_millis(millis),
                        result,
                        table: TranspositionTableStatistics {
                            replacements: millis,
                            ..TranspositionTableStatistics::default()
                        },
                    }
                })
                .collect();
            let median = median_sample(&mut samples);
            assert_eq!(median.elapsed, Duration::from_millis(expected));
            assert_eq!(median.result.statistics.nodes, expected * 100);
            assert_eq!(median.result.score, expected as i32);
            assert_eq!(median.table.replacements, expected);
        }
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
