use rustmoku_core::{Game, GameStatus, Move, MoveError, OPENINGS, RecordError, Stone};

fn at(text: &str) -> Move {
    text.parse().unwrap()
}

#[test]
fn history_and_repeated_undo_restore_every_position_field_and_terminal_status() {
    let mut game = Game::default();
    let mut positions = vec![game.position().clone()];
    let sequence = ["D8", "A1", "E8", "C1", "F8", "E1", "G8", "G1", "H8"];
    for text in sequence {
        game.play_move(at(text)).unwrap();
        positions.push(game.position().clone());
    }
    assert_eq!(game.status(), GameStatus::Won(Stone::Black));
    assert_eq!(game.history().collect::<Vec<_>>(), sequence.map(at));
    assert_eq!(game.play_move(at("I8")), Err(MoveError::GameOver));
    assert_eq!(game.history().len(), 9);
    assert_eq!(game.undo(), Some(at("H8")));
    assert_eq!(game.status(), GameStatus::Ongoing);
    assert_eq!(game.position(), &positions[8]);
    assert_eq!(
        game.play_move(at("D8")),
        Err(MoveError::Occupied { at: at("D8") })
    );
    assert_eq!(game.history().len(), 8);
    assert_eq!(game.undo_plies(2), 2);
    assert_eq!(game.position(), &positions[6]);
    while game.undo().is_some() {
        assert_eq!(game.position(), &positions[game.history().len()]);
    }
    assert_eq!(game.undo_plies(usize::MAX), 0);
    assert_eq!(game, Game::default());
    game.play_move(at("H8")).unwrap();
    assert_eq!(game.history().collect::<Vec<_>>(), [Move::CENTER]);
}

#[test]
fn shared_notation_maps_corners_center_and_all_valid_moves() {
    for (text, row, col) in [
        ("A1", 14, 0),
        ("O1", 14, 14),
        ("A15", 0, 0),
        ("O15", 0, 14),
        ("H8", 7, 7),
        ("I8", 7, 8),
    ] {
        let expected = Move::from_row_col(row, col).unwrap();
        assert_eq!(at(text), expected);
        assert_eq!(at(&text.to_lowercase()), expected);
        assert_eq!(expected.to_string(), text);
    }
    for mv in Move::all() {
        assert_eq!(at(&mv.to_string()), mv);
    }
    for bad in ["", "A0", "P1", "A16", "H08", "1A", "A-1", "А1", "A١", "HH8"] {
        assert!(bad.parse::<Move>().is_err(), "{bad}");
    }
}

#[test]
fn records_round_trip_by_legal_replay_and_reject_invalid_sequences() {
    let text = "RustMoku 1\nrules=freestyle\nmoves=h8 H9 g8 I8\n";
    let mut game = Game::from_record(text).unwrap();
    assert_eq!(
        game.to_record(),
        "RustMoku 1\nrules=freestyle\nmoves=H8 H9 G8 I8\n"
    );
    assert_eq!(Game::from_record(&game.to_record()).unwrap(), game);
    assert_eq!(
        Game::from_record(&game.to_record().replace('\n', "\r\n")).unwrap(),
        game
    );
    game.undo_plies(2);
    assert_eq!(Game::from_record(&game.to_record()).unwrap(), game);
    let won = Game::from_record("RustMoku 1\nrules=freestyle\nmoves=D8 A1 E8 C1 F8 E1 G8 G1 H8\n")
        .unwrap();
    assert_eq!(won.status(), GameStatus::Won(Stone::Black));
    assert_eq!(Game::from_record(&won.to_record()).unwrap(), won);
    assert!(matches!(
        Game::from_record(&text.replace("RustMoku 1", "RustMoku 2")),
        Err(RecordError::UnsupportedVersion(_))
    ));
    assert!(matches!(
        Game::from_record(&text.replace("freestyle", "renju")),
        Err(RecordError::UnsupportedRules(_))
    ));
    assert!(matches!(
        Game::from_record(&text.replace("I8", "P8")),
        Err(RecordError::InvalidCoordinate { ply: 4, .. })
    ));
    assert!(matches!(
        Game::from_record(&text.replace("I8", "H8")),
        Err(RecordError::IllegalMove {
            ply: 4,
            source: MoveError::Occupied { .. },
            ..
        })
    ));
    assert!(matches!(
        Game::from_record(&format!("{} I8\n", won.to_record().trim_end())),
        Err(RecordError::IllegalMove {
            ply: 10,
            source: MoveError::GameOver,
            ..
        })
    ));
    assert!(Game::from_record("RustMoku 1\nrules=freestyle\nmoves=\nunknown=1").is_err());
    assert_eq!(
        Game::from_record(&Game::default().to_record()).unwrap(),
        Game::default()
    );
}

#[test]
fn built_in_openings_have_unique_ids_and_replay_as_complete_records() {
    assert_eq!(OPENINGS.len(), 12);
    for (index, opening) in OPENINGS.iter().enumerate() {
        assert!(OPENINGS[..index].iter().all(|other| opening.id != other.id));
        let game = opening.game().unwrap();
        assert_eq!(game.status(), GameStatus::Ongoing);
        assert_eq!(game.history().collect::<Vec<_>>(), opening.moves);
        assert_eq!(game.position().rules(), opening.rules);
        assert_eq!(Game::from_record(&game.to_record()).unwrap(), game);
    }
}
