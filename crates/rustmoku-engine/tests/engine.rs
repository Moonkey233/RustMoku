use rustmoku_core::{Move, Position, Stone};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, EngineConfig, Evaluator, SearchEngine, SearchLimits,
};

fn move_at(row: usize, column: usize) -> Move {
    Move::from_row_col(row, column).expect("test coordinates must be valid")
}

fn play(position: &mut Position, row: usize, column: usize) {
    position
        .make_move(move_at(row, column))
        .expect("test move must be legal");
}

struct ZeroEvaluator;

impl Evaluator for ZeroEvaluator {
    fn evaluate(&self, _position: &Position) -> i32 {
        0
    }
}

fn test_engine() -> AlphaBetaEngine {
    AlphaBetaEngine::with_config(ClassicalEvaluator, EngineConfig::new(1))
}

#[test]
fn empty_board_generates_and_selects_center() {
    let position = Position::default();
    let result = test_engine().search(&position, SearchLimits::new(2));
    assert_eq!(result.best_move, Some(Move::CENTER));
}

#[test]
fn equal_root_scores_choose_the_canonical_smallest_move() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(1));

    let result = engine.search(&position, SearchLimits::new(1));

    assert_eq!(result.best_move, Some(move_at(5, 5)));
}

#[test]
fn evaluator_uses_side_to_move_perspective() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 0, 0);
    play(&mut position, 7, 8);

    assert_eq!(position.side_to_move(), Stone::White);
    assert!(ClassicalEvaluator.evaluate(&position) < 0);
}

#[test]
fn search_selects_an_immediate_win() {
    let mut position = Position::default();
    for (black_column, white_row, white_column) in [(3, 0, 0), (4, 0, 2), (5, 1, 0), (6, 1, 2)] {
        play(&mut position, 7, black_column);
        play(&mut position, white_row, white_column);
    }

    let result = test_engine().search(&position, SearchLimits::new(1));
    let best = result.best_move.expect("a winning move must exist");
    assert!(position.would_win(best, Stone::Black));
    assert!(result.score > 0);
}

#[test]
fn search_blocks_the_opponents_immediate_win() {
    let mut position = Position::default();
    for (black, white_column) in [((7, 2), 3), ((0, 0), 4), ((0, 2), 5), ((1, 0), 6)] {
        play(&mut position, black.0, black.1);
        play(&mut position, 7, white_column);
    }

    let result = test_engine().search(&position, SearchLimits::new(2));
    assert_eq!(result.best_move, Some(move_at(7, 7)));
}

#[test]
fn search_does_not_mutate_its_input() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    play(&mut position, 8, 8);
    let before = position.clone();

    let _result = test_engine().search(&position, SearchLimits::new(2));

    assert_eq!(position, before);
}

#[test]
fn iterative_deepening_completes_requested_depth_and_reports_seldepth() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    let result = test_engine().search(&position, SearchLimits::new(3));

    assert_eq!(result.requested_depth, 3);
    assert_eq!(result.completed_depth, 3);
    assert_eq!(result.seldepth, 3);
}

#[test]
fn principal_variation_starts_with_best_move_and_is_legal() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    play(&mut position, 8, 8);
    play(&mut position, 7, 8);
    let result = test_engine().search(&position, SearchLimits::new(3));

    assert_eq!(
        result.principal_variation.first().copied(),
        result.best_move
    );
    let mut replay = position.clone();
    for at in result.principal_variation {
        assert!(replay.is_legal(at), "PV move {} must be legal", at.index());
        replay.make_move(at).expect("validated PV move must apply");
    }
}

#[test]
fn fresh_engines_have_deterministic_semantic_results() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    play(&mut position, 8, 8);
    play(&mut position, 7, 8);
    let limits = SearchLimits::new(2);

    let cold_a = test_engine().search(&position, limits);
    let cold_b = test_engine().search(&position, limits);
    assert_eq!(cold_a.best_move, cold_b.best_move);
    assert_eq!(cold_a.score, cold_b.score);
    assert_eq!(cold_a.completed_depth, cold_b.completed_depth);
}

#[test]
fn warm_engine_preserves_semantic_result_and_records_tt_hits() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    play(&mut position, 8, 8);
    play(&mut position, 7, 8);
    let mut engine = test_engine();
    let limits = SearchLimits::new(3);

    let cold = engine.search(&position, limits);
    let warm = engine.search(&position, limits);
    assert_eq!(cold.best_move, warm.best_move);
    assert_eq!(cold.score, warm.score);
    assert_eq!(cold.completed_depth, warm.completed_depth);
    assert!(warm.statistics.tt_hits > 0);
    assert!(warm.statistics.tt_cutoffs > 0);
}
