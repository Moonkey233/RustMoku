//! Dedicated four-way DFPN cache. Entries never cross public-search generations.
use rustmoku_core::{Move, Stone};

use super::threat::ThreatDescriptor;
use crate::board_state::BoardState;

pub(super) const INFINITY: u32 = u32::MAX / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Numbers {
    pub(super) proof: u32,
    pub(super) disproof: u32,
}

impl Numbers {
    pub(super) const UNKNOWN: Self = Self {
        proof: 1,
        disproof: 1,
    };
    pub(super) const WIN: Self = Self {
        proof: 0,
        disproof: INFINITY,
    };
    pub(super) const NO_PROOF: Self = Self {
        proof: INFINITY,
        disproof: 0,
    };

    pub(super) fn solved(self) -> bool {
        self.proof == 0 || self.disproof == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TacticalKey {
    position: u64,
    context: u64,
    attacker: Stone,
    defender: bool,
}

impl TacticalKey {
    pub(super) fn new(
        board: &BoardState,
        attacker: Stone,
        active: Option<ThreatDescriptor>,
    ) -> Self {
        Self {
            position: board.key().value(),
            context: active.map_or(0, ThreatDescriptor::signature),
            attacker,
            defender: board.position().side_to_move() != attacker,
        }
    }

    fn index(self, mask: usize) -> usize {
        (self.position
            ^ self.context.rotate_left(23)
            ^ (self.attacker as u64)
            ^ u64::from(self.defender)) as usize
            & mask
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Entry {
    key: TacticalKey,
    generation: u64,
    pub(super) numbers: Numbers,
    pub(super) best_move: Option<Move>,
    pub(super) depth: u8,
    /// Present only after canonical minimax certificate reconstruction. A DFPN
    /// first proof alone does not establish shortest game-theoretic distance.
    pub(super) distance: Option<u8>,
}

impl Entry {
    const EMPTY: Self = Self {
        key: TacticalKey {
            position: 0,
            context: 0,
            attacker: Stone::Black,
            defender: false,
        },
        generation: 0,
        numbers: Numbers::UNKNOWN,
        best_move: None,
        depth: 0,
        distance: None,
    };
}

const WAYS: usize = 4;
pub(super) struct Table {
    buckets: Vec<[Entry; WAYS]>,
    generation: u64,
}

impl Table {
    pub(super) fn new(memory_mib: usize) -> Self {
        let desired = memory_mib.saturating_mul(1024 * 1024) / std::mem::size_of::<[Entry; WAYS]>();
        let count = 1_usize << desired.max(1).ilog2();
        Self {
            buckets: vec![[Entry::EMPTY; WAYS]; count],
            generation: 0,
        }
    }

    pub(super) fn begin_search(&mut self) {
        self.generation = match self.generation.checked_add(1) {
            Some(next) => next,
            None => {
                self.buckets.fill([Entry::EMPTY; WAYS]);
                1
            }
        };
    }

    pub(super) fn probe(&self, key: TacticalKey, depth: u8) -> Option<Entry> {
        self.buckets[key.index(self.buckets.len() - 1)]
            .iter()
            .copied()
            .find(|entry| {
                self.generation != 0
                    && entry.generation == self.generation
                    && entry.key == key
                    && entry.depth == depth
            })
    }

    pub(super) fn store(
        &mut self,
        key: TacticalKey,
        depth: u8,
        numbers: Numbers,
        best_move: Option<Move>,
        distance: Option<u8>,
    ) {
        let index = key.index(self.buckets.len() - 1);
        let bucket = &mut self.buckets[index];
        let same = bucket
            .iter()
            .position(|e| e.generation == self.generation && e.key == key && e.depth == depth);
        let slot = same
            .or_else(|| bucket.iter().position(|e| e.generation != self.generation))
            .unwrap_or_else(|| {
                (0..WAYS)
                    .min_by_key(|&i| (bucket[i].numbers.solved(), bucket[i].depth, i))
                    .expect("nonempty bucket")
            });
        if same.is_some() && bucket[slot].numbers.solved() && !numbers.solved() {
            return;
        }
        bucket[slot] = Entry {
            key,
            generation: self.generation,
            depth,
            numbers,
            best_move,
            distance,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bitboard::BitBoard256, pattern::ThreatProfile};

    #[test]
    fn context_depth_full_key_and_generation_isolate_entries() {
        let board = BoardState::new(&rustmoku_core::Position::default());
        let mut threat = ThreatDescriptor {
            gain: Move::CENTER,
            kind: ThreatProfile::OpenThree,
            continuations: BitBoard256::EMPTY,
            defenses: BitBoard256::EMPTY,
            dependencies: BitBoard256::EMPTY,
        };
        let first = TacticalKey::new(&board, Stone::Black, Some(threat));
        threat.defenses.set(Move::CENTER);
        let second = TacticalKey::new(&board, Stone::Black, Some(threat));
        let mut table = Table::new(0);
        table.begin_search();
        table.store(first, 5, Numbers::WIN, Some(Move::CENTER), Some(5));
        assert!(table.probe(second, 5).is_none());
        assert!(table.probe(first, 3).is_none());
        assert!(
            table
                .probe(TacticalKey::new(&board, Stone::White, Some(threat)), 5)
                .is_none()
        );
        let mut collision = first;
        collision.position ^= 1 << 32;
        assert!(table.probe(collision, 5).is_none());
        assert_eq!(table.probe(first, 5).unwrap().distance, Some(5));
        table.begin_search();
        assert!(table.probe(first, 5).is_none());
        table.generation = u64::MAX;
        table.begin_search();
        assert!(table.probe(first, 5).is_none());
        assert_eq!(std::mem::size_of::<Entry>(), 48);
        assert_eq!(
            Table::new(16).buckets.len() * std::mem::size_of::<[Entry; WAYS]>(),
            12 * 1024 * 1024
        );
    }
}
