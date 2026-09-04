use rustmoku_core::{CELL_COUNT, Move, Position, Stone};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, Evaluator, SearchEngine, SearchLimits, generate_candidates,
};

fn move_at(row: usize, column: usize) -> Move {
    Move::from_row_col(row, column).expect("test coordinates must be valid")
}

fn play(position: &mut Position, row: usize, column: usize) {
    position
        .make_move(move_at(row, column))
        .expect("test move must be legal");
}

#[test]
fn empty_board_generates_and_selects_center() {
    let position = Position::default();
    let candidates = generate_candidates(&position);
    assert_eq!(candidates.iter().collect::<Vec<_>>(), vec![Move::CENTER]);

    let result = AlphaBetaEngine::default().search(&position, SearchLimits::new(2));
    assert_eq!(result.best_move, Some(Move::CENTER));
}

#[test]
fn generated_candidates_are_legal_and_unique() {
    let mut position = Position::default();
    play(&mut position, 0, 0);
    play(&mut position, 14, 14);
    play(&mut position, 7, 7);

    let candidates = generate_candidates(&position);
    let mut seen = [false; CELL_COUNT];
    for at in candidates.iter() {
        assert!(position.is_legal(at));
        assert!(!seen[at.index()], "candidate {} was duplicated", at.index());
        seen[at.index()] = true;
    }
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

    let result = AlphaBetaEngine::default().search(&position, SearchLimits::new(1));
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

    let result = AlphaBetaEngine::default().search(&position, SearchLimits::new(2));
    assert_eq!(result.best_move, Some(move_at(7, 7)));
}

#[test]
fn search_does_not_mutate_its_input() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    play(&mut position, 8, 8);
    let before = position.clone();

    let _result = AlphaBetaEngine::default().search(&position, SearchLimits::new(2));

    assert_eq!(position, before);
}

#[test]
fn identical_searches_are_deterministic() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 7);
    play(&mut position, 8, 8);
    play(&mut position, 7, 8);
    let engine = AlphaBetaEngine::default();
    let limits = SearchLimits::new(2);

    assert_eq!(
        engine.search(&position, limits),
        engine.search(&position, limits)
    );
}
