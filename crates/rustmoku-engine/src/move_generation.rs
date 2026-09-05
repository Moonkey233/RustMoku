#[cfg(any(test, feature = "bench-internals"))]
use rustmoku_core::Position;
use rustmoku_core::{CELL_COUNT, Move};

/// Fixed-capacity move storage sized to the natural board maximum.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MoveList {
    moves: [Move; CELL_COUNT],
    len: usize,
}

impl MoveList {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            moves: [Move::CENTER; CELL_COUNT],
            len: 0,
        }
    }

    #[must_use]
    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn iter(&self) -> impl ExactSizeIterator<Item = Move> + '_ {
        self.as_slice().iter().copied()
    }

    pub(crate) const fn as_slice(&self) -> &[Move] {
        self.moves.split_at(self.len).0
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [Move] {
        self.moves.split_at_mut(self.len).0
    }

    pub(crate) fn push(&mut self, at: Move) {
        debug_assert!(self.len < CELL_COUNT);
        self.moves[self.len] = at;
        self.len += 1;
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

/// Generates every empty point within Chebyshev distance two of a stone.
/// The empty-board exception is the center point. Results are unique and in
/// ascending move-index order.
#[cfg(any(test, feature = "bench-internals"))]
#[must_use]
pub(crate) fn generate_candidates(position: &Position) -> MoveList {
    let mut candidates = MoveList::new();
    if position.winner().is_some() || position.is_full() {
        return candidates;
    }
    if position.move_count() == 0 {
        candidates.push(Move::CENTER);
        return candidates;
    }

    let mut nearby = [false; CELL_COUNT];
    for occupied in Move::all().filter(|&at| position.cell(at).is_some()) {
        for row_delta in -2_isize..=2 {
            for column_delta in -2_isize..=2 {
                let Some(row) = occupied.row().checked_add_signed(row_delta) else {
                    continue;
                };
                let Some(column) = occupied.column().checked_add_signed(column_delta) else {
                    continue;
                };
                let Ok(at) = Move::from_row_col(row, column) else {
                    continue;
                };
                nearby[at.index()] = true;
            }
        }
    }

    for at in Move::all() {
        // Terminal positions returned above, so emptiness is the only remaining
        // legality condition and avoids repeating winner detection per cell.
        if nearby[at.index()] && position.cell(at).is_none() {
            candidates.push(at);
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    use rustmoku_core::Position;
    use rustmoku_core::{CELL_COUNT, Move};

    use super::generate_candidates;

    fn move_at(row: usize, column: usize) -> Move {
        Move::from_row_col(row, column).expect("test coordinates must be valid")
    }

    #[test]
    fn empty_board_generates_only_center() {
        let candidates = generate_candidates(&Position::default());
        assert_eq!(candidates.as_slice(), &[Move::CENTER]);
    }

    #[test]
    fn candidates_are_legal_and_unique() {
        let mut position = Position::default();
        for at in [move_at(0, 0), move_at(14, 14), move_at(7, 7)] {
            position.make_move(at).expect("test move must be legal");
        }

        let candidates = generate_candidates(&position);
        let mut seen = [false; CELL_COUNT];
        for at in candidates.iter() {
            assert!(position.is_legal(at));
            assert!(!seen[at.index()], "candidate {} was duplicated", at.index());
            seen[at.index()] = true;
        }
    }
}
