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
}

/// Opaque state required to reverse exactly one successful move.
#[derive(Debug)]
pub struct MoveUndo {
    played: Move,
    stone: Stone,
    previous_last_move: Option<Move>,
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
        self.cells[at.index()].is_none() && self.move_count < CELL_COUNT && self.winner().is_none()
    }

    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.move_count == CELL_COUNT
    }

    /// Applies one legal move and returns the only token that can restore it.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::GameOver`] for a terminal position and
    /// [`MoveError::Occupied`] when `at` already contains a stone.
    pub fn make_move(&mut self, at: Move) -> Result<MoveUndo, MoveError> {
        if self.winner().is_some() || self.is_full() {
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
        };
        self.cells[at.index()] = Some(stone);
        self.move_count += 1;
        self.last_move = Some(at);
        self.side_to_move = stone.opponent();
        Ok(undo)
    }

    /// Reverses the most recent move.
    ///
    /// `MoveUndo` is intentionally non-cloneable and this method consumes it.
    /// Passing a token to a different position or using tokens out of LIFO order
    /// is a programmer error and triggers an assertion before mutation.
    pub fn unmake_move(&mut self, undo: MoveUndo) {
        assert_eq!(
            self.last_move,
            Some(undo.played),
            "moves must be unmade in LIFO order"
        );
        assert_eq!(
            self.cell(undo.played),
            Some(undo.stone),
            "undo token does not match the board"
        );
        assert_eq!(
            self.side_to_move,
            undo.stone.opponent(),
            "undo token does not match the side to move"
        );
        assert!(
            self.move_count > 0,
            "a non-empty undo token requires a played move"
        );

        self.cells[undo.played.index()] = None;
        self.move_count -= 1;
        self.last_move = undo.previous_last_move;
        self.side_to_move = undo.stone;
    }

    /// Returns the winner created by the last move, if any.
    #[must_use]
    pub fn winner(&self) -> Option<Stone> {
        let last_move = self.last_move?;
        let stone = self.cell(last_move)?;
        self.has_five_from(last_move, stone).then_some(stone)
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
