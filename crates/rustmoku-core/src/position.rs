use crate::{CELL_COUNT, Move, MoveError, RuleSet, Stone};

const DIRECTIONS: [(isize, isize); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];

/// The complete rule-relevant state consumed by the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    cells: [Option<Stone>; CELL_COUNT],
    side_to_move: Stone,
    rules: RuleSet,
    move_count: usize,
    last_move: Option<Move>,
    winner: Option<Stone>,
    // Analysis nesting is bounded; keep the counter in Position padding.
    analysis_depth: u8,
}

/// Opaque state required to reverse exactly one successful move.
#[derive(Debug)]
pub struct MoveUndo {
    played: Move,
    stone: Stone,
    previous_last_move: Option<Move>,
    previous_winner: Option<Stone>,
}

/// Opaque undo for an analysis-only turn change. This is not a played Move,
/// is never recorded by Game, and must be restored after all child moves.
#[derive(Debug)]
#[doc(hidden)]
#[must_use = "restore the analysis turn after unmaking its children"]
pub struct AnalysisTurnUndo {
    side: Stone,
    count: usize,
    depth: u8,
    last: Option<Move>,
}

impl Position {
    #[must_use]
    pub const fn new(rules: RuleSet) -> Self {
        Self {
            cells: [None; CELL_COUNT],
            side_to_move: Stone::Black,
            rules,
            move_count: 0,
            last_move: None,
            winner: None,
            analysis_depth: 0,
        }
    }

    #[must_use]
    pub const fn side_to_move(&self) -> Stone {
        self.side_to_move
    }

    #[must_use]
    pub const fn rules(&self) -> RuleSet {
        self.rules
    }

    #[must_use]
    pub const fn move_count(&self) -> usize {
        self.move_count
    }

    #[must_use]
    pub const fn last_move(&self) -> Option<Move> {
        self.last_move
    }

    #[must_use]
    pub const fn cell(&self, at: Move) -> Option<Stone> {
        self.cells[at.index()]
    }

    #[must_use]
    pub fn is_legal(&self, at: Move) -> bool {
        self.cells[at.index()].is_none() && self.move_count < CELL_COUNT && self.winner.is_none()
    }

    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.move_count == CELL_COUNT
    }

    /// Temporarily selects the opponent for hypothetical analysis, without a
    /// stone, history entry or change to terminal status. This state is not a
    /// reachable game record. Callers must isolate all derived evidence and
    /// restore in strict LIFO order. Game deliberately exposes no such operation.
    /// Internal engine contract: never pass this hypothetical Position to Game,
    /// canonical records, proof solvers/books or ordinary search entry points.
    /// Cross-crate visibility exists solely for reversible engine analysis.
    #[doc(hidden)]
    pub fn begin_analysis_opponent_turn(&mut self) -> Result<AnalysisTurnUndo, MoveError> {
        if self.winner.is_some() || self.is_full() {
            return Err(MoveError::GameOver);
        }
        let undo = AnalysisTurnUndo {
            side: self.side_to_move,
            count: self.move_count,
            depth: self.analysis_depth,
            last: self.last_move,
        };
        self.analysis_depth = self
            .analysis_depth
            .checked_add(1)
            .expect("analysis nesting overflow");
        self.side_to_move = self.side_to_move.opponent();
        Ok(undo)
    }
    /// Restores a hypothetical turn after every child has been unmade.
    #[doc(hidden)]
    pub fn end_analysis_opponent_turn(&mut self, undo: AnalysisTurnUndo) {
        assert_eq!(
            self.move_count, undo.count,
            "analysis children must be unmade"
        );
        assert_eq!(
            self.last_move, undo.last,
            "analysis children must be unmade"
        );
        assert_eq!(
            self.side_to_move,
            undo.side.opponent(),
            "analysis turn LIFO"
        );
        assert!(self.winner.is_none(), "analysis children must be unmade");
        assert_eq!(self.analysis_depth, undo.depth + 1, "analysis turn LIFO");
        self.analysis_depth = undo.depth;
        self.side_to_move = undo.side;
    }

    /// Applies one legal move and returns the only token that can restore it.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::GameOver`] for a terminal position and
    /// [`MoveError::Occupied`] when `at` already contains a stone.
    pub fn make_move(&mut self, at: Move) -> Result<MoveUndo, MoveError> {
        if self.winner.is_some() || self.is_full() {
            return Err(MoveError::GameOver);
        }
        if self.cell(at).is_some() {
            return Err(MoveError::Occupied { at });
        }

        let stone = self.side_to_move;
        let undo = MoveUndo {
            played: at,
            stone,
            previous_last_move: self.last_move,
            previous_winner: self.winner,
        };
        self.cells[at.index()] = Some(stone);
        self.move_count += 1;
        self.last_move = Some(at);
        self.side_to_move = stone.opponent();
        self.winner = self.has_five_from(at, stone).then_some(stone);
        Ok(undo)
    }

    /// Reverses the most recent move.
    ///
    /// `MoveUndo` is opaque and intentionally non-cloneable. It must be applied
    /// to its corresponding logical position in strict LIFO order. Violating
    /// that contract is a programmer error; debug builds detect common misuse,
    /// but tokens do not carry enough identity to detect every cross-position
    /// misuse.
    pub fn unmake_move(&mut self, undo: MoveUndo) {
        debug_assert_eq!(
            self.last_move,
            Some(undo.played),
            "moves must be unmade in LIFO order"
        );
        debug_assert_eq!(
            self.cell(undo.played),
            Some(undo.stone),
            "undo token does not match the board"
        );
        debug_assert_eq!(
            self.side_to_move,
            undo.stone.opponent(),
            "undo token does not match the side to move"
        );
        debug_assert!(
            self.move_count > 0,
            "a non-empty undo token requires a played move"
        );

        self.cells[undo.played.index()] = None;
        self.move_count -= 1;
        self.last_move = undo.previous_last_move;
        self.side_to_move = undo.stone;
        self.winner = undo.previous_winner;
    }

    /// Returns the winner created by the last move, if any.
    #[must_use]
    pub const fn winner(&self) -> Option<Stone> {
        self.winner
    }

    /// Tests whether placing `stone` at an empty location would win under the
    /// active rule set. This keeps rule knowledge out of engine move ordering.
    #[must_use]
    pub fn would_win(&self, at: Move, stone: Stone) -> bool {
        if self.cell(at).is_some() {
            return false;
        }

        match self.rules {
            RuleSet::Freestyle => DIRECTIONS.into_iter().any(|(row_step, column_step)| {
                1 + self.count_direction(at, stone, row_step, column_step)
                    + self.count_direction(at, stone, -row_step, -column_step)
                    >= 5
            }),
        }
    }

    fn has_five_from(&self, at: Move, stone: Stone) -> bool {
        match self.rules {
            RuleSet::Freestyle => DIRECTIONS.into_iter().any(|(row_step, column_step)| {
                1 + self.count_direction(at, stone, row_step, column_step)
                    + self.count_direction(at, stone, -row_step, -column_step)
                    >= 5
            }),
        }
    }

    fn count_direction(
        &self,
        from: Move,
        stone: Stone,
        row_step: isize,
        column_step: isize,
    ) -> usize {
        let mut row = from.row();
        let mut column = from.column();
        let mut count = 0;

        while let Some(next_row) = row.checked_add_signed(row_step) {
            let Some(next_column) = column.checked_add_signed(column_step) else {
                break;
            };
            let Ok(next_move) = Move::from_row_col(next_row, next_column) else {
                break;
            };
            if self.cell(next_move) != Some(stone) {
                break;
            }

            count += 1;
            row = next_row;
            column = next_column;
        }

        count
    }
}

impl Default for Position {
    fn default() -> Self {
        Self::new(RuleSet::Freestyle)
    }
}

#[cfg(test)]
mod analysis_turn_tests {
    use super::*;
    #[test]
    #[should_panic(expected = "analysis turn LIFO")]
    fn out_of_order_analysis_tokens_are_rejected_even_with_matching_side() {
        let mut p = Position::default();
        let first = p.begin_analysis_opponent_turn().unwrap();
        let _second = p.begin_analysis_opponent_turn().unwrap();
        let _third = p.begin_analysis_opponent_turn().unwrap();
        p.end_analysis_opponent_turn(first);
    }
    #[test]
    fn hypothetical_turn_is_not_a_move_and_real_children_restore() {
        let mut p = Position::default();
        p.make_move(Move::CENTER).unwrap();
        let original = p.clone();
        let turn = p.begin_analysis_opponent_turn().unwrap();
        assert_eq!(p.move_count(), original.move_count());
        assert_eq!(p.last_move(), original.last_move());
        let at = Move::from_index(0).unwrap();
        let child = p.make_move(at).unwrap();
        assert_eq!(p.cell(at), Some(Stone::Black));
        p.unmake_move(child);
        p.end_analysis_opponent_turn(turn);
        assert_eq!(p, original);
    }
}
