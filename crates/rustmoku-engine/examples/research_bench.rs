//! Small bounded F-stage matrix driver. Timings are diagnostics, never strength.
use rustmoku_core::{Move, Position};
use rustmoku_engine::{
    AlphaBetaEngine, EngineConfig, ProofLimits, RuntimeEvaluator, SearchEngine, SearchLimits,
};
use std::{
    error::Error,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut evaluator = RuntimeEvaluator::Pattern;
    let mut threads = 1;
    let mut mode = String::from("depth");
    let mut probe = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => {
                evaluator = RuntimeEvaluator::read_from_path(args.next().ok_or("missing model")?)?;
            }
            "--threads" => threads = args.next().ok_or("missing threads")?.parse()?,
            "--mode" => mode = args.next().ok_or("missing mode")?,
            "--probe" => probe = true,
            _ => return Err("unknown option".into()),
        }
    }
    if !(1..=8).contains(&threads) {
        return Err("smoke supports 1..8 workers".into());
    }
    let limits = match mode.as_str() {
        "depth" => SearchLimits::new(4),
        "work" => SearchLimits::new(8).with_max_nodes(20_000),
        "time" => SearchLimits::new(12).with_move_time(Duration::from_millis(20)),
        _ => return Err("mode must be depth/work/time".into()),
    };
    let mut config = EngineConfig::new(1).with_threads(threads);
    if probe {
        config = config.with_interior_vcf(ProofLimits::new(5, 240), 960);
    }
    let mut engine = AlphaBetaEngine::with_config(evaluator, config);
    let mut position = Position::default();
    for index in [112, 97, 128, 113] {
        position.make_move(Move::from_index(index)?)?;
    }
    println!(
        "mode,threads,tt,elapsed_ms,depth,score,best,work,principal_nodes,helper_nodes,tt_hits,tt_cutoffs,tt_stores,tt_bytes,proof_attempts,proof_work,proof_success,proof_ns,termination"
    );
    for tt in ["cold", "warm"] {
        let start = Instant::now();
        let result = engine.search(&position, limits);
        let elapsed = start.elapsed();
        let s = result.statistics;
        println!(
            "{mode},{threads},{tt},{:.3},{},{},{:?},{},{},{},{},{},{},{},{},{},{},{},{:?}",
            elapsed.as_secs_f64() * 1000.0,
            result.completed_depth,
            result.score,
            result.best_move.map(|at| at.index()),
            s.work_nodes,
            s.principal_nodes,
            s.helper_nodes,
            s.tt_hits,
            s.tt_cutoffs,
            s.tt_stores,
            engine.transposition_table_statistics().allocated_bytes,
            s.interior_proof.attempts,
            s.interior_proof.work,
            s.interior_proof.proven,
            s.interior_proof.elapsed_nanos,
            result.termination
        );
    }
    Ok(())
}
