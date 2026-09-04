use rustmoku_core::{CELL_COUNT, Move};

const PLY_ROWS: usize = CELL_COUNT + 1;

pub(crate) struct PvTable {
    moves: [[Move; CELL_COUNT]; PLY_ROWS],
    lengths: [usize; PLY_ROWS],
}

impl PvTable {
    pub(crate) const fn new() -> Self {
        Self {
            moves: [[Move::CENTER; CELL_COUNT]; PLY_ROWS],
            lengths: [0; PLY_ROWS],
        }
    }

    pub(crate) fn clear(&mut self, ply: u8) {
        self.lengths[usize::from(ply)] = 0;
    }

    pub(crate) fn update(&mut self, ply: u8, at: Move) {
        let row = usize::from(ply);
        debug_assert!(row < CELL_COUNT);
        let child_length = self.lengths[row + 1].min(CELL_COUNT - 1);
        self.moves[row][0] = at;
        for index in 0..child_length {
            let child_move = self.moves[row + 1][index];
            self.moves[row][index + 1] = child_move;
        }
        self.lengths[row] = child_length + 1;
    }

    pub(crate) fn root_line(&self) -> &[Move] {
        self.moves[0].split_at(self.lengths[0]).0
    }
}

#[cfg(test)]
mod tests {
    use rustmoku_core::Move;

    use super::PvTable;

    #[test]
    fn parent_line_copies_child_without_allocation() {
        let mut pv = PvTable::new();
        let first = Move::from_index(1).expect("test move must be valid");
        let second = Move::from_index(2).expect("test move must be valid");
        pv.clear(1);
        pv.update(1, second);
        pv.clear(0);
        pv.update(0, first);
        assert_eq!(pv.root_line(), &[first, second]);
    }
}
