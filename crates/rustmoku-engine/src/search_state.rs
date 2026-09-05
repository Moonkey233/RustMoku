use rustmoku_core::{Move, MoveError, MoveUndo, Position, Stone};

use crate::{
    Evaluator, PatternState, PatternUndo, candidate_frontier::CandidateFrontier,
    move_generation::MoveList, zobrist::PositionKey,
};

pub(crate) struct SearchState<E: Evaluator> {
    position: Position,
    key: PositionKey,
    frontier: CandidateFrontier,
    evaluator_state: E::State,
    patterns: PatternState,
}

pub(crate) struct SearchUndo<U> {
    evaluator_undo: U,
    pattern_undo: PatternUndo,
    position_undo: MoveUndo,
    played: Move,
    stone: Stone,
}

impl<E: Evaluator> SearchState<E> {
    pub(crate) fn new(position: &Position, evaluator: &E) -> Self {
        // This is the one intentional Position clone in a public search.
        let position = position.clone();
        let key = PositionKey::from_position(&position);
        let frontier = CandidateFrontier::new(&position);
        let evaluator_state = evaluator.initialize(&position);
        let patterns = PatternState::new(&position);
        Self {
            evaluator_state,
            patterns,
            position,
            key,
            frontier,
        }
    }

    pub(crate) const fn position(&self) -> &Position {
        &self.position
    }

    pub(crate) const fn key(&self) -> PositionKey {
        self.key
    }

    pub(crate) fn candidates(&self) -> MoveList {
        if self.position.winner().is_some() || self.position.is_full() {
            MoveList::new()
        } else {
            self.frontier.candidates()
        }
    }

    pub(crate) fn candidate_bits(&self) -> crate::bitboard::BitBoard256 {
        if self.position.winner().is_some() || self.position.is_full() {
            crate::bitboard::BitBoard256::EMPTY
        } else {
            self.frontier.candidate_bits()
        }
    }

    pub(crate) fn evaluate(&self, evaluator: &E) -> i32 {
        evaluator.evaluate(&self.position, &self.patterns, &self.evaluator_state)
    }

    pub(crate) fn patterns(&self) -> &PatternState {
        &self.patterns
    }

    pub(crate) fn make_move(
        &mut self,
        at: Move,
        evaluator: &E,
    ) -> Result<SearchUndo<E::Undo>, MoveError> {
        let stone = self.position.side_to_move();
        let position_undo = self.position.make_move(at)?;
        self.frontier.make_move(at);
        let evaluator_undo = evaluator.make_move(&mut self.evaluator_state, at, stone);
        let pattern_undo = self.patterns.make_move(at, stone);
        self.key = self.key.toggle_move(at, stone);
        debug_assert_eq!(self.key, PositionKey::from_position(&self.position));
        Ok(SearchUndo {
            evaluator_undo,
            pattern_undo,
            position_undo,
            played: at,
            stone,
        })
    }

    pub(crate) fn unmake_move(&mut self, undo: SearchUndo<E::Undo>, evaluator: &E) {
        evaluator.unmake_move(&mut self.evaluator_state, undo.evaluator_undo);
        self.patterns.unmake_move(undo.pattern_undo);
        self.position.unmake_move(undo.position_undo);
        self.frontier.unmake_move(undo.played);
        self.key = self.key.toggle_move(undo.played, undo.stone);
        debug_assert_eq!(self.key, PositionKey::from_position(&self.position));
    }

    #[cfg(test)]
    pub(crate) fn assert_consistent(&self, evaluator: &E)
    where
        E::State: std::fmt::Debug + PartialEq,
    {
        assert_eq!(self.key, PositionKey::from_position(&self.position));
        assert_eq!(self.frontier, CandidateFrontier::new(&self.position));
        assert_eq!(
            self.candidates(),
            crate::move_generation::generate_candidates(&self.position)
        );
        assert_eq!(self.patterns(), &PatternState::reference(&self.position));
        let reference = evaluator.initialize(&self.position);
        assert_eq!(self.evaluator_state, reference);
        assert_eq!(
            self.evaluate(evaluator),
            evaluator.evaluate(
                &self.position,
                &PatternState::reference(&self.position),
                &reference
            )
        );
    }
}

#[cfg(test)]
mod tests {
    use rustmoku_core::{Move, Position};

    use super::SearchState;
    use crate::ClassicalEvaluator;
    use crate::zobrist::PositionKey;

    fn move_at(row: usize, column: usize) -> Move {
        Move::from_row_col(row, column).expect("test coordinates must be valid")
    }

    #[test]
    fn long_make_unmake_sequence_restores_position_and_hash() {
        let original = Position::default();
        let mut state = SearchState::new(&original, &ClassicalEvaluator);
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
                    .make_move(move_at(row, column), &ClassicalEvaluator)
                    .expect("test sequence must remain legal"),
            );
            assert_eq!(state.key(), PositionKey::from_position(state.position()));
        }
        while let Some(undo) = undos.pop() {
            state.unmake_move(undo, &ClassicalEvaluator);
            assert_eq!(state.key(), PositionKey::from_position(state.position()));
        }

        assert_eq!(state.position(), &original);
        assert_eq!(state.key(), original_key);
    }

    fn exercise_lifecycle<E: crate::Evaluator>(evaluator: E)
    where
        E::State: std::fmt::Debug + PartialEq,
    {
        let mut seed = 0x4d595df4d0f33173_u64;
        for _ in 0..16 {
            let original = Position::default();
            let mut state = SearchState::new(&original, &evaluator);
            let mut undos = Vec::new();
            for _ in 0..180 {
                let legal: Vec<_> = Move::all()
                    .filter(|&at| state.position().is_legal(at))
                    .collect();
                if legal.is_empty() {
                    break;
                }
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = legal[(seed % legal.len() as u64) as usize];
                undos.push(state.make_move(at, &evaluator).unwrap());
                state.assert_consistent(&evaluator);
                // Occupied or terminal rejection cannot partly update sidecars.
                assert!(state.make_move(at, &evaluator).is_err());
                state.assert_consistent(&evaluator);
            }
            while let Some(undo) = undos.pop() {
                state.unmake_move(undo, &evaluator);
                state.assert_consistent(&evaluator);
            }
            assert_eq!(state.position(), &original);
        }
    }

    #[test]
    fn pattern_lifecycle_coordinates_every_sidecar_and_failed_moves_are_atomic() {
        exercise_lifecycle(crate::PatternEvaluator);
        assert_eq!(
            std::mem::size_of::<<crate::PatternEvaluator as crate::Evaluator>::State>(),
            0
        );
        assert_eq!(
            std::mem::size_of::<<crate::PatternEvaluator as crate::Evaluator>::Undo>(),
            0
        );
        let size = std::mem::size_of::<SearchState<crate::PatternEvaluator>>();
        assert_eq!(size, std::mem::size_of::<SearchState<ClassicalEvaluator>>());
        assert_eq!(
            size,
            3768 + 2
                * crate::pattern::ThreatProfile::COUNT
                * std::mem::size_of::<crate::bitboard::BitBoard256>()
        );
        println!("SearchState<PatternEvaluator>: V0.4=3768, V0.5={size} bytes");
    }

    #[test]
    fn classical_unit_state_shares_the_engine_tactical_state() {
        exercise_lifecycle(ClassicalEvaluator);
    }

    #[test]
    fn full_draw_has_no_candidates_and_unmakes_back_to_empty() {
        let original = Position::default();
        let evaluator = crate::PatternEvaluator;
        let mut state = SearchState::new(&original, &evaluator);
        // BBWW down columns, alternating across rows: neither diagonal can
        // contain five. Any subset also has no five, so every prefix is legal.
        let mut black = Move::all().filter(|at| (at.row() + 2 * at.column()) % 4 < 2);
        let mut white = Move::all().filter(|at| (at.row() + 2 * at.column()) % 4 >= 2);
        let mut undos = Vec::new();
        for ply in 0..rustmoku_core::CELL_COUNT {
            let at = if ply % 2 == 0 {
                black.next()
            } else {
                white.next()
            }
            .unwrap();
            undos.push(state.make_move(at, &evaluator).unwrap());
            state.assert_consistent(&evaluator);
        }
        assert!(state.position().is_full());
        assert_eq!(state.position().winner(), None);
        assert!(state.candidates().is_empty());
        while let Some(undo) = undos.pop() {
            state.unmake_move(undo, &evaluator);
            state.assert_consistent(&evaluator);
        }
        assert_eq!(state.position(), &original);
    }
}
