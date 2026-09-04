use rustmoku_core::{Move, MoveError, MoveUndo, Position, Stone};

use crate::zobrist::PositionKey;

pub(crate) struct SearchState {
    position: Position,
    key: PositionKey,
}

pub(crate) struct SearchUndo {
    position_undo: MoveUndo,
    played: Move,
    stone: Stone,
}

impl SearchState {
    pub(crate) fn new(position: &Position) -> Self {
        // This is the one intentional Position clone in a public search.
        let position = position.clone();
        let key = PositionKey::from_position(&position);
        Self { position, key }
    }

    pub(crate) const fn position(&self) -> &Position {
        &self.position
    }

    pub(crate) const fn key(&self) -> PositionKey {
        self.key
    }

    pub(crate) fn make_move(&mut self, at: Move) -> Result<SearchUndo, MoveError> {
        let stone = self.position.side_to_move();
        let position_undo = self.position.make_move(at)?;
        self.key = self.key.toggle_move(at, stone);
        debug_assert_eq!(self.key, PositionKey::from_position(&self.position));
        Ok(SearchUndo {
            position_undo,
            played: at,
            stone,
        })
    }

    pub(crate) fn unmake_move(&mut self, undo: SearchUndo) {
        self.position.unmake_move(undo.position_undo);
        self.key = self.key.toggle_move(undo.played, undo.stone);
        debug_assert_eq!(self.key, PositionKey::from_position(&self.position));
    }
}

#[cfg(test)]
mod tests {
    use rustmoku_core::{Move, Position};

    use super::SearchState;
    use crate::zobrist::PositionKey;

    fn move_at(row: usize, column: usize) -> Move {
        Move::from_row_col(row, column).expect("test coordinates must be valid")
    }

    #[test]
    fn long_make_unmake_sequence_restores_position_and_hash() {
        let original = Position::default();
        let mut state = SearchState::new(&original);
        let original_key = state.key();
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
                state
                    .make_move(move_at(row, column))
                    .expect("test sequence must remain legal"),
            );
            assert_eq!(state.key(), PositionKey::from_position(state.position()));
        }
        while let Some(undo) = undos.pop() {
            state.unmake_move(undo);
            assert_eq!(state.key(), PositionKey::from_position(state.position()));
        }

        assert_eq!(state.position(), &original);
        assert_eq!(state.key(), original_key);
    }
}
