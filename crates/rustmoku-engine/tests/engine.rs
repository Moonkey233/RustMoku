use rustmoku_core::{Move, Position, Stone};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, EngineConfig, Evaluator, PatternEvaluator, SearchEngine,
    SearchLimits,
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
    type State = ();
    type Undo = ();
    fn initialize(&self, _position: &Position) {}
    fn make_move(&self, _state: &mut (), _at: Move, _stone: rustmoku_core::Stone) {}
    fn unmake_move(&self, _state: &mut (), _undo: ()) {}
    fn evaluate(
        &self,
        _position: &Position,
        _patterns: &rustmoku_engine::PatternState,
        _state: &(),
    ) -> i32 {
        0
    }
}

fn test_engine() -> AlphaBetaEngine {
    AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1))
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
    assert!(
        ClassicalEvaluator.evaluate(
            &position,
            &rustmoku_engine::PatternState::new(&position),
            &()
        ) < 0
    );
}

#[test]
fn pattern_evaluator_uses_side_to_move_perspective() {
    let mut position = Position::default();
    for (row, column) in [(7, 6), (0, 0), (7, 7), (0, 2), (7, 8)] {
        play(&mut position, row, column);
    }
    assert_eq!(position.side_to_move(), Stone::White);
    let patterns = rustmoku_engine::PatternState::new(&position);
    assert!(PatternEvaluator.evaluate(&position, &patterns, &()) < 0);
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
    assert!((3..=9).contains(&result.seldepth));
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

#[test]
fn broken_four_gap_is_selected_as_an_immediate_win() {
    let mut position = Position::default();
    for (index, col) in [4, 5, 7, 8].into_iter().enumerate() {
        play(&mut position, 7, col);
        play(&mut position, 0, index * 2);
    }
    for mut engine in [test_engine(), test_engine()] {
        let before = position.clone();
        let cold = engine.search(&position, SearchLimits::new(3));
        let warm = engine.search(&position, SearchLimits::new(3));
        assert_eq!(cold.best_move, Some(move_at(7, 6)));
        assert_eq!(cold.best_move, warm.best_move);
        assert_eq!(cold.score, 99_999_999);
        assert_eq!(cold.score, warm.score);
        assert_eq!(position, before);
    }
}

#[test]
fn classical_reference_remains_constructible_and_tactically_correct() {
    let mut position = Position::default();
    for (index, col) in [4, 5, 7, 8].into_iter().enumerate() {
        play(&mut position, 7, col);
        play(&mut position, 0, index * 2);
    }
    let mut engine = AlphaBetaEngine::with_config(ClassicalEvaluator, EngineConfig::new(1));
    let result = engine.search(&position, SearchLimits::new(2));
    assert_eq!(result.best_move, Some(move_at(7, 6)));
    assert_eq!(result.score, 99_999_999);
}

#[test]
fn terminal_and_zero_depth_searches_resolve_tactical_scores() {
    let mut position = Position::default();
    for (index, col) in [4, 5, 7, 8].into_iter().enumerate() {
        play(&mut position, 7, col);
        play(&mut position, 0, index * 2);
    }
    let static_result = test_engine().search(&position, SearchLimits::new(0));
    assert_eq!(static_result.score, 99_999_999);
    assert_eq!(static_result.best_move, None);
    play(&mut position, 7, 6);
    let terminal = test_engine().search(&position, SearchLimits::new(5));
    assert_eq!(terminal.best_move, None);
    assert_eq!(terminal.score, -100_000_000);
    assert_eq!(terminal.completed_depth, 0);
    assert_eq!(terminal.statistics.static_evaluations, 0);
}

#[test]
fn deeper_child_cache_cannot_change_a_shallower_ancestor_search() {
    let ancestor = Position::default();
    let mut child = ancestor.clone();
    play(&mut child, 7, 7);
    let limits = SearchLimits::new(4);
    let cold = test_engine().search(&ancestor, limits);
    let mut warm_engine = test_engine();
    warm_engine.search(&child, limits);
    let warm = warm_engine.search(&ancestor, limits);
    assert_eq!((warm.best_move, warm.score), (cold.best_move, cold.score));
}
