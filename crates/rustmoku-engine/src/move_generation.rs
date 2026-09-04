use rustmoku_core::{CELL_COUNT, Move, Position};

/// Fixed-capacity move storage sized to the natural board maximum.
#[derive(Debug, PartialEq, Eq)]
pub struct MoveList {
    moves: [Option<Move>; CELL_COUNT],
    len: usize,
}

impl MoveList {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            moves: [None; CELL_COUNT],
            len: 0,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = Move> + '_ {
        self.moves[..self.len].iter().filter_map(|entry| *entry)
    }

    pub(crate) fn get(&self, index: usize) -> Option<Move> {
        self.moves.get(index).copied().flatten()
    }

    pub(crate) fn swap(&mut self, left: usize, right: usize) {
        self.moves.swap(left, right);
    }

    fn push(&mut self, at: Move) {
        debug_assert!(self.len < CELL_COUNT);
        self.moves[self.len] = Some(at);
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
#[must_use]
pub fn generate_candidates(position: &Position) -> MoveList {
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
