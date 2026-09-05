use rustmoku_core::{BOARD_SIZE, CELL_COUNT, Move, Position};

use crate::{
    bitboard::{BitBoard256, MOVES},
    move_generation::MoveList,
};

pub(crate) static RADIUS2_MASKS: [BitBoard256; CELL_COUNT] = radius_masks();

const fn radius_masks() -> [BitBoard256; CELL_COUNT] {
    let mut masks = [BitBoard256::EMPTY; CELL_COUNT];
    let mut index = 0;
    while index < CELL_COUNT {
        let at = MOVES[index];
        let mut other = 0;
        while other < CELL_COUNT {
            if at.row().abs_diff(other / BOARD_SIZE) <= 2
                && at.column().abs_diff(other % BOARD_SIZE) <= 2
            {
                masks[index].set(MOVES[other]);
            }
            other += 1;
        }
        index += 1;
    }
    masks
}

/// Counts include occupied cells and the center itself. At most 25 stones
/// contribute to a cell, so u8 cannot overflow under legal make/unmake.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CandidateFrontier {
    occupied: BitBoard256,
    frontier: BitBoard256,
    neighbor_counts: [u8; CELL_COUNT],
}

impl CandidateFrontier {
    pub(crate) fn new(position: &Position) -> Self {
        let mut state = Self {
            occupied: BitBoard256::EMPTY,
            frontier: BitBoard256::EMPTY,
            neighbor_counts: [0; CELL_COUNT],
        };
        for at in Move::all().filter(|&at| position.cell(at).is_some()) {
            state.make_move(at);
        }
        state
    }

    pub(crate) fn make_move(&mut self, at: Move) {
        debug_assert!(!self.occupied.test(at));
        let neighbors = RADIUS2_MASKS[at.index()];
        for neighbor in neighbors.iter() {
            self.neighbor_counts[neighbor.index()] += 1;
        }
        // The union is equivalent to setting just 0 -> 1 counts, with four ORs.
        self.frontier = self.frontier.union(neighbors);
        self.occupied.set(at);
    }

    pub(crate) fn unmake_move(&mut self, at: Move) {
        debug_assert!(self.occupied.test(at));
        self.occupied.clear(at);
        for neighbor in RADIUS2_MASKS[at.index()].iter() {
            let count = &mut self.neighbor_counts[neighbor.index()];
            debug_assert!(*count > 0);
            *count -= 1;
            if *count == 0 {
                self.frontier.clear(neighbor);
            }
        }
    }

    pub(crate) fn candidate_bits(&self) -> BitBoard256 {
        if self.occupied.is_empty() {
            let mut center = BitBoard256::EMPTY;
            center.set(Move::CENTER);
            center
        } else {
            self.frontier
                .and_not(self.occupied)
                .intersection(BitBoard256::PLAYABLE)
        }
    }

    pub(crate) fn candidates(&self) -> MoveList {
        let mut moves = MoveList::new();
        for at in self.candidate_bits().iter() {
            moves.push(at);
        }
        moves
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateFrontier, RADIUS2_MASKS};
    use crate::{bitboard::BitBoard256, move_generation::generate_candidates};
    use rustmoku_core::{Move, Position};

    #[test]
    fn all_radius_masks_match_independent_geometry() {
        for at in Move::all() {
            let expected: Vec<_> = Move::all()
                .filter(|other| {
                    at.row().abs_diff(other.row()) <= 2 && at.column().abs_diff(other.column()) <= 2
                })
                .collect();
            let mask = RADIUS2_MASKS[at.index()];
            assert_eq!(mask.iter().collect::<Vec<_>>(), expected);
            assert!(mask.and_not(BitBoard256::PLAYABLE).is_empty());
        }
        assert_eq!(RADIUS2_MASKS[0].iter().count(), 9);
        assert_eq!(RADIUS2_MASKS[7].iter().count(), 15);
        assert_eq!(RADIUS2_MASKS[112].iter().count(), 25);
    }

    #[test]
    fn overlapping_neighbors_survive_undo() {
        let mut position = Position::default();
        let first = Move::CENTER;
        let second = Move::from_index(113).unwrap();
        position.make_move(first).unwrap();
        let mut state = CandidateFrontier::new(&position);
        let original = CandidateFrontier::new(&position);
        state.make_move(second);
        assert_eq!(state.neighbor_counts[first.index()], 2);
        state.unmake_move(second);
        assert_eq!(state, original);
    }

    #[test]
    fn deterministic_sequences_match_v02_reference_and_restore_every_field() {
        let mut seed = 0x1234_5678_u64;
        for _ in 0..48 {
            let mut position = Position::default();
            let mut state = CandidateFrontier::new(&position);
            let original = CandidateFrontier::new(&position);
            let mut undos = Vec::new();
            for _ in 0..160 {
                let legal: Vec<_> = Move::all().filter(|&at| position.is_legal(at)).collect();
                if legal.is_empty() {
                    break;
                }
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let at = legal[(seed % legal.len() as u64) as usize];
                undos.push((at, position.make_move(at).unwrap()));
                state.make_move(at);
                assert_eq!(state, CandidateFrontier::new(&position));
                if position.winner().is_none() {
                    let actual = state.candidates();
                    assert_eq!(actual, generate_candidates(&position));
                    assert!(actual.as_slice().windows(2).all(|pair| pair[0] < pair[1]));
                    assert!(actual.iter().all(|at| position.is_legal(at)));
                }
            }
            while let Some((at, undo)) = undos.pop() {
                position.unmake_move(undo);
                state.unmake_move(at);
                assert_eq!(state, CandidateFrontier::new(&position));
                assert_eq!(state.candidates(), generate_candidates(&position));
            }
            assert_eq!(state, original);
        }
    }
}
