use crate::{
    ClassicalEvaluator, Evaluator, LearnedModelError, PatternEvaluator, PatternState,
    RuntimeEvaluator, candidate_frontier::CandidateFrontier, move_generation::generate_candidates,
    move_ordering::order_moves, search_state::SearchState,
};
use rustmoku_core::{Move, Position};
use std::{hint::black_box, path::Path, time::Instant};

/// Runs one warm-up and five samples per operation on the historical balanced
/// midgame. Reports the median nanoseconds per call/pair. No timing assertions.
pub fn run_hotpath(iterations: usize) {
    eprintln!(
        "Layout (bytes): BitBoard={}, CandidateFrontier={}, PatternState={}, PatternUndo={}, SearchState<PatternEvaluator>={}",
        std::mem::size_of::<crate::bitboard::BitBoard256>(),
        std::mem::size_of::<CandidateFrontier>(),
        std::mem::size_of::<PatternState>(),
        std::mem::size_of::<crate::pattern_state::PatternUndo>(),
        std::mem::size_of::<SearchState<PatternEvaluator>>(),
    );
    let mut position = Position::default();
    for (row, column) in [
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
    ] {
        position
            .make_move(Move::from_row_col(row, column).expect("fixture coordinates are valid"))
            .expect("fixture moves are legal");
    }
    let mut frontier = CandidateFrontier::new(&position);
    let mut patterns = PatternState::new(&position);
    let mut state = SearchState::new(&position, &PatternEvaluator);
    let moves = state.candidates();
    let side = position.side_to_move();
    println!("operation,iterations,repeats,median_ns");
    measure("candidate_reference", iterations, |_| {
        black_box(generate_candidates(black_box(&position)));
    });
    measure("candidate_incremental", iterations, |_| {
        black_box(black_box(&frontier).candidates());
    });
    measure("frontier_make_unmake_pair", iterations, |index| {
        let at = moves.as_slice()[index % moves.as_slice().len()];
        frontier.make_move(at);
        black_box(&frontier);
        frontier.unmake_move(at);
    });
    measure("pattern_full_initialize", iterations, |_| {
        black_box(PatternState::new(black_box(&position)));
    });
    measure("pattern_make_unmake_pair", iterations, |index| {
        let at = moves.as_slice()[index % moves.as_slice().len()];
        let undo = patterns.make_move(at, side);
        black_box(&patterns);
        patterns.unmake_move(undo);
    });
    measure("classical_evaluate", iterations, |_| {
        black_box(ClassicalEvaluator.evaluate(black_box(&position), &patterns, &()));
    });
    measure("pattern_evaluate", iterations, |_| {
        black_box(PatternEvaluator.evaluate(black_box(&position), black_box(&patterns), &()));
    });
    measure("search_state_make_unmake_pair", iterations, |index| {
        let at = moves.as_slice()[index % moves.as_slice().len()];
        let undo = state
            .make_move(at, &PatternEvaluator)
            .expect("fixture candidate must be legal");
        black_box(&state);
        state.unmake_move(undo, &PatternEvaluator);
    });
    measure("candidates_and_ordering", iterations, |_| {
        let mut moves = state.candidates();
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            None,
            &crate::search_heuristics::SearchHeuristics::default(),
            0,
            |_| None,
        );
        black_box(moves);
    });
}

/// Measures the production scalar quantized Value/Policy and reversible state
/// update paths using an explicitly supplied model artifact.
pub fn run_learned_hotpath(
    iterations: usize,
    model_path: impl AsRef<Path>,
) -> Result<(), LearnedModelError> {
    match RuntimeEvaluator::read_from_path(model_path)? {
        RuntimeEvaluator::Learned(evaluator) => learned_hotpath(iterations, evaluator),
        RuntimeEvaluator::Nonlinear(evaluator) => learned_hotpath(iterations, evaluator),
        RuntimeEvaluator::MixLite(evaluator) => learned_hotpath(iterations, evaluator),
        RuntimeEvaluator::Pattern => unreachable!("model reader does not return Pattern"),
    }
    Ok(())
}

fn learned_hotpath<E: Evaluator>(iterations: usize, evaluator: E) {
    let mut position = Position::default();
    for (row, column) in [
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
    ] {
        position
            .make_move(Move::from_row_col(row, column).expect("fixture coordinates are valid"))
            .expect("fixture moves are legal");
    }
    let patterns = PatternState::new(&position);
    measure("learned_initialize", iterations.min(1000), |_| {
        black_box(evaluator.initialize(black_box(&position), black_box(&patterns)));
    });
    eprintln!(
        "evaluator_state_inline_bytes={}",
        std::mem::size_of::<E::State>()
    );
    let evaluator_state = evaluator.initialize(&position, &patterns);
    let mut search_state = SearchState::new(&position, &evaluator);
    let moves = search_state.candidates();
    println!("operation,iterations,repeats,median_ns");
    measure("learned_value", iterations, |_| {
        black_box(evaluator.evaluate(
            black_box(&position),
            black_box(&patterns),
            black_box(&evaluator_state),
        ));
    });
    measure("learned_policy_candidate", iterations, |index| {
        let at = moves.as_slice()[index % moves.as_slice().len()];
        black_box(evaluator.policy_score(
            black_box(&position),
            black_box(&patterns),
            black_box(&evaluator_state),
            at,
        ));
    });
    measure("learned_make_unmake_pair", iterations, |index| {
        let at = moves.as_slice()[index % moves.as_slice().len()];
        let undo = search_state
            .make_move(at, &evaluator)
            .expect("fixture candidate must be legal");
        black_box(&search_state);
        search_state.unmake_move(undo, &evaluator);
    });
}

fn measure(name: &str, iterations: usize, mut operation: impl FnMut(usize)) {
    for index in 0..iterations {
        operation(index);
    }
    let mut samples = [0.0; 5];
    for sample in &mut samples {
        let started = Instant::now();
        for index in 0..iterations {
            operation(index);
        }
        *sample = started.elapsed().as_secs_f64() * 1e9 / iterations as f64;
    }
    samples.sort_unstable_by(f64::total_cmp);
    println!("{name},{iterations},5,{:.2}", samples[2]);
}
