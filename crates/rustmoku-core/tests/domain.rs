use rustmoku_core::{
    BOARD_SIZE, CELL_COUNT, Game, GameStatus, Move, MoveError, Position, RuleSet, Stone,
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
fn move_coordinates_and_indices_round_trip() {
    for index in 0..CELL_COUNT {
        let at = Move::from_index(index).expect("every board index is valid");
        assert_eq!(at.index(), index);
        assert_eq!(Move::from_row_col(at.row(), at.column()), Ok(at));
    }
    assert_eq!(Move::all().count(), CELL_COUNT);
    assert_eq!(Move::CENTER, move_at(BOARD_SIZE / 2, BOARD_SIZE / 2));
}

#[test]
fn invalid_moves_are_rejected() {
    assert_eq!(
        Move::from_row_col(BOARD_SIZE, 0),
        Err(MoveError::OutOfBounds {
            row: BOARD_SIZE,
            column: 0,
        })
    );
    assert_eq!(
        Move::from_row_col(0, BOARD_SIZE),
        Err(MoveError::OutOfBounds {
            row: 0,
            column: BOARD_SIZE,
        })
    );
    assert_eq!(
        Move::from_index(CELL_COUNT),
        Err(MoveError::IndexOutOfBounds { index: CELL_COUNT })
    );
}

#[test]
fn stones_have_opponents() {
    assert_eq!(Stone::Black.opponent(), Stone::White);
    assert_eq!(Stone::White.opponent(), Stone::Black);
}

#[test]
fn a_new_position_is_empty() {
    let position = Position::default();
    assert_eq!(position.move_count(), 0);
    assert_eq!(position.side_to_move(), Stone::Black);
    assert_eq!(position.last_move(), None);
    assert_eq!(position.winner(), None);
    assert!(Move::all().all(|at| position.cell(at).is_none()));
}

#[test]
fn legal_move_switches_side_and_occupied_move_is_rejected() {
    let mut position = Position::default();
    let at = move_at(7, 7);
    position.make_move(at).expect("center must be legal");

    assert_eq!(position.cell(at), Some(Stone::Black));
    assert_eq!(position.move_count(), 1);
    assert_eq!(position.last_move(), Some(at));
    assert_eq!(position.side_to_move(), Stone::White);
    assert!(matches!(
        position.make_move(at),
        Err(MoveError::Occupied { at: occupied }) if occupied == at
    ));
}

#[test]
fn make_and_unmake_restore_the_exact_position() {
    let mut position = Position::default();
    play(&mut position, 7, 7);
    play(&mut position, 6, 6);
    let before = position.clone();

    let undo = position
        .make_move(move_at(8, 8))
        .expect("move must be legal");
    position.unmake_move(undo);

    assert_eq!(position, before);
}

#[test]
fn winning_move_sets_and_unmake_restores_cached_winner() {
    let mut position = Position::default();
    for column in 0..4 {
        play(&mut position, 7, column);
        play(&mut position, 0, column);
    }
    let before_win = position.clone();

    let undo = position
        .make_move(move_at(7, 4))
        .expect("fifth stone must be legal");
    assert_eq!(position.winner(), Some(Stone::Black));
    assert!(matches!(
        position.make_move(move_at(8, 8)),
        Err(MoveError::GameOver)
    ));

    position.unmake_move(undo);
    assert_eq!(position.winner(), None);
    assert_eq!(position, before_win);
}

#[test]
fn long_make_unmake_sequence_restores_exact_position() {
    let mut position = Position::default();
    let original = position.clone();
    let sequence = [
        (7, 7),
        (6, 7),
        (8, 8),
        (7, 8),
        (8, 7),
        (6, 8),
        (9, 6),
        (5, 9),
        (9, 8),
        (5, 7),
        (6, 9),
        (8, 6),
        (10, 5),
        (4, 10),
        (10, 9),
        (4, 6),
    ];
    let mut undos = Vec::with_capacity(sequence.len());

    for (row, column) in sequence {
        undos.push(
            position
                .make_move(move_at(row, column))
                .expect("test sequence must remain legal"),
        );
    }
    while let Some(undo) = undos.pop() {
        position.unmake_move(undo);
    }

    assert_eq!(position, original);
}

#[test]
fn detects_horizontal_five() {
    let mut game = Game::default();
    for column in 0..4 {
        game.play_move(move_at(7, column)).expect("black move");
        game.play_move(move_at(0, column)).expect("white move");
    }
    game.play_move(move_at(7, 4)).expect("winning move");
    assert_eq!(game.status(), GameStatus::Won(Stone::Black));
    assert!(!game.position().is_legal(move_at(8, 8)));
    assert_eq!(game.play_move(move_at(8, 8)), Err(MoveError::GameOver));
}

#[test]
fn detects_vertical_five() {
    let mut game = Game::default();
    for row in 0..4 {
        game.play_move(move_at(row, 7)).expect("black move");
        game.play_move(move_at(row, 0)).expect("white move");
    }
    game.play_move(move_at(4, 7)).expect("winning move");
    assert_eq!(game.status(), GameStatus::Won(Stone::Black));
}

#[test]
fn detects_backslash_diagonal_five() {
    let mut game = Game::default();
    for offset in 0..4 {
        game.play_move(move_at(offset, offset)).expect("black move");
        game.play_move(move_at(offset, 10)).expect("white move");
    }
    game.play_move(move_at(4, 4)).expect("winning move");
    assert_eq!(game.status(), GameStatus::Won(Stone::Black));
}

#[test]
fn detects_slash_diagonal_five() {
    let mut game = Game::default();
    for offset in 0..4 {
        game.play_move(move_at(4 - offset, offset))
            .expect("black move");
        game.play_move(move_at(offset, 10)).expect("white move");
    }
    game.play_move(move_at(0, 4)).expect("winning move");
    assert_eq!(game.status(), GameStatus::Won(Stone::Black));
}

#[test]
fn four_stones_do_not_win() {
    let mut position = Position::default();
    for column in 0..4 {
        play(&mut position, 7, column);
        play(&mut position, 0, column);
    }
    assert_eq!(position.winner(), None);
}

#[test]
fn freestyle_bridge_move_detects_six_as_a_win() {
    let mut game = Game::new(RuleSet::Freestyle);
    for column in [0, 1, 2, 4, 5] {
        game.play_move(move_at(7, column)).expect("black move");
        game.play_move(move_at(0, column)).expect("white move");
    }
    game.play_move(move_at(7, 3)).expect("six-forming move");
    assert_eq!(game.status(), GameStatus::Won(Stone::Black));
}
