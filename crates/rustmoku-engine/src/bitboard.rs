use rustmoku_core::{CELL_COUNT, Move};

/// Engine-private 15x15 bit set. Bits 225..256 are always zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BitBoard256([u64; 4]);

impl BitBoard256 {
    pub(crate) const EMPTY: Self = Self([0; 4]);
    pub(crate) const PLAYABLE: Self = Self([u64::MAX, u64::MAX, u64::MAX, (1 << 33) - 1]);

    pub(crate) const fn set(&mut self, at: Move) {
        self.0[at.index() >> 6] |= 1 << (at.index() & 63);
    }

    pub(crate) fn clear(&mut self, at: Move) {
        self.0[at.index() >> 6] &= !(1 << (at.index() & 63));
    }

    pub(crate) const fn test(&self, at: Move) -> bool {
        self.0[at.index() >> 6] & (1 << (at.index() & 63)) != 0
    }

    pub(crate) const fn union(self, other: Self) -> Self {
        Self([
            self.0[0] | other.0[0],
            self.0[1] | other.0[1],
            self.0[2] | other.0[2],
            self.0[3] | other.0[3],
        ])
    }

    pub(crate) const fn intersection(self, other: Self) -> Self {
        Self([
            self.0[0] & other.0[0],
            self.0[1] & other.0[1],
            self.0[2] & other.0[2],
            self.0[3] & other.0[3],
        ])
    }

    pub(crate) const fn and_not(self, other: Self) -> Self {
        Self([
            self.0[0] & !other.0[0],
            self.0[1] & !other.0[1],
            self.0[2] & !other.0[2],
            self.0[3] & !other.0[3],
        ])
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == [0; 4]
    }

    pub(crate) fn iter(self) -> SetBits {
        SetBits {
            bits: self,
            word: 0,
        }
    }
}

pub(crate) struct SetBits {
    bits: BitBoard256,
    word: usize,
}

impl Iterator for SetBits {
    type Item = Move;

    #[inline]
    fn next(&mut self) -> Option<Move> {
        while self.word < 4 {
            let bits = &mut self.bits.0[self.word];
            if *bits != 0 {
                let index = self.word * 64 + bits.trailing_zeros() as usize;
                *bits &= *bits - 1;
                return Some(MOVES[index]);
            }
            self.word += 1;
        }
        None
    }
}

/// Validated at compile time so bit iteration needs no Result construction.
pub(crate) const MOVES: [Move; CELL_COUNT] = {
    let mut moves = [Move::CENTER; CELL_COUNT];
    let mut index = 0;
    while index < CELL_COUNT {
        moves[index] = match Move::from_index(index) {
            Ok(at) => at,
            Err(_) => panic!("compile-time board index must be valid"),
        };
        index += 1;
    }
    moves
};

#[cfg(test)]
mod tests {
    use super::{BitBoard256, MOVES};

    #[test]
    fn set_clear_test_and_bitwise_operations_cover_every_word() {
        let mut evens = BitBoard256::EMPTY;
        let mut odds = BitBoard256::EMPTY;
        for at in MOVES {
            if at.index() % 2 == 0 {
                evens.set(at);
            } else {
                odds.set(at);
            }
        }
        assert!(evens.intersection(odds).is_empty());
        assert_eq!(evens.union(odds), BitBoard256::PLAYABLE);
        assert_eq!(BitBoard256::PLAYABLE.and_not(evens), odds);
        for at in MOVES {
            assert_eq!(evens.test(at), at.index() % 2 == 0);
            evens.clear(at);
        }
        assert!(evens.is_empty());
    }

    #[test]
    fn ascending_iteration_includes_last_playable_cell_but_no_padding() {
        assert_eq!(BitBoard256::PLAYABLE.iter().collect::<Vec<_>>(), MOVES);
        assert_eq!(BitBoard256::PLAYABLE.0[3] >> 33, 0);
        let mut last = BitBoard256::EMPTY;
        last.set(MOVES[224]);
        assert_eq!(last.iter().collect::<Vec<_>>(), [MOVES[224]]);
        assert_eq!(BitBoard256::EMPTY.iter().next(), None);
        assert_eq!(std::mem::size_of::<BitBoard256>(), 32);
    }
}
