use rustmoku_core::{Move, MoveError, Position, Stone};

use crate::{
    Evaluator, PatternState,
    board_state::{BoardState, BoardUndo},
    move_generation::MoveList,
    vcf::{VcfResult, VcfSolver},
    zobrist::PositionKey,
};

pub(crate) struct SearchState<E: Evaluator> {
    board: BoardState,
    evaluator_state: E::State,
}

pub(crate) struct SearchUndo<U> {
    board: BoardUndo,
    evaluator: U,
}

impl<E: Evaluator> SearchState<E> {
    pub(crate) fn new(position: &Position, evaluator: &E) -> Self {
        Self {
            board: BoardState::new(position),
            evaluator_state: evaluator.initialize(position),
        }
    }

    pub(crate) fn position(&self) -> &Position {
        self.board.position()
    }
    pub(crate) fn key(&self) -> PositionKey {
        self.board.key()
    }
    pub(crate) fn patterns(&self) -> &PatternState {
        self.board.patterns()
    }
    pub(crate) fn candidates(&self) -> MoveList {
        self.board.candidates()
    }
    pub(crate) fn candidate_bits(&self) -> crate::bitboard::BitBoard256 {
        self.board.candidate_bits()
    }

    pub(crate) fn evaluate(&self, evaluator: &E) -> i32 {
        evaluator.evaluate(self.position(), self.patterns(), &self.evaluator_state)
    }

    /// The solver restores the board before returning an owned result. No mutable
    /// board getter or callback can expose a board/accumulator mismatch.
    pub(crate) fn prove_vcf(
        &mut self,
        solver: &mut VcfSolver,
        attacker: Stone,
        max_plies: u8,
        budget: &mut crate::search_control::SearchBudget,
    ) -> VcfResult {
        solver.solve_controlled(&mut self.board, attacker, max_plies, budget)
    }

    pub(crate) fn prove_vct(
        &mut self,
        solver: &mut crate::vct::VctSolver,
        attacker: Stone,
        max_plies: u8,
        budget: &mut crate::search_control::SearchBudget,
    ) -> crate::vct::VctResult {
        solver.solve_controlled(&mut self.board, attacker, max_plies, budget)
    }

    pub(crate) fn make_move(
        &mut self,
        at: Move,
        evaluator: &E,
    ) -> Result<SearchUndo<E::Undo>, MoveError> {
        let stone = self.position().side_to_move();
        let board = self.board.make_move(at)?;
        let evaluator = evaluator.make_move(&mut self.evaluator_state, at, stone);
        Ok(SearchUndo { board, evaluator })
    }

    pub(crate) fn unmake_move(&mut self, undo: SearchUndo<E::Undo>, evaluator: &E) {
        evaluator.unmake_move(&mut self.evaluator_state, undo.evaluator);
        self.board.unmake_move(undo.board);
    }

    #[cfg(test)]
    pub(crate) fn assert_consistent(&self, evaluator: &E)
    where
        E::State: std::fmt::Debug + PartialEq,
    {
        self.board.assert_consistent();
        let reference = evaluator.initialize(self.position());
        assert_eq!(self.evaluator_state, reference);
        assert_eq!(
            self.evaluate(evaluator),
            evaluator.evaluate(
                self.position(),
                &PatternState::reference(self.position()),
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
        println!("SearchState<PatternEvaluator>: {size} bytes");
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
