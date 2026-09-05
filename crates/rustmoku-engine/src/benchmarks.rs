use crate::{
    ClassicalEvaluator, Evaluator, PatternEvaluator, PatternState,
    candidate_frontier::CandidateFrontier, move_generation::generate_candidates,
    move_ordering::order_moves, search_state::SearchState,
};
use rustmoku_core::{Move, Position};
use std::{hint::black_box, time::Instant};

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
        );
        black_box(moves);
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
