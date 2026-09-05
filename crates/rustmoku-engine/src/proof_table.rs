use rustmoku_core::{Move, Stone};

use crate::zobrist::PositionKey;

const BUCKET_COUNT: usize = 4096;
const WAYS: usize = 4;

/// Budget exhaustion cannot be represented in the proof cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CachedProof {
    NotProven,
    ProvenWin { plies: u8 },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProofEntry {
    key: u64,
    generation: u64,
    pub(crate) depth: u8,
    pub(crate) proof: CachedProof,
    pub(crate) best_move: Option<Move>,
}

impl Default for ProofEntry {
    fn default() -> Self {
        Self {
            key: 0,
            generation: 0,
            depth: 0,
            proof: CachedProof::NotProven,
            best_move: None,
        }
    }
}

/// Allocation persists; entries are usable only in the current public search.
pub(crate) struct ProofTable {
    buckets: Vec<[ProofEntry; WAYS]>,
    generation: u64,
}

impl ProofTable {
    pub(crate) fn new() -> Self {
        Self {
            buckets: vec![[ProofEntry::default(); WAYS]; BUCKET_COUNT],
            generation: 0,
        }
    }

    pub(crate) fn begin_search(&mut self) {
        self.generation = match self.generation.checked_add(1) {
            Some(generation) => generation,
            None => {
                // Only on u64 exhaustion, never an O(capacity) per-search clear.
                self.buckets.fill([ProofEntry::default(); WAYS]);
                1
            }
        };
    }

    pub(crate) fn probe(&self, key: u64) -> Option<ProofEntry> {
        self.buckets[key as usize & (BUCKET_COUNT - 1)]
            .iter()
            .copied()
            .find(|entry| {
                self.generation != 0 && entry.generation == self.generation && entry.key == key
            })
    }

    pub(crate) fn store(
        &mut self,
        key: u64,
        depth: u8,
        proof: CachedProof,
        best_move: Option<Move>,
    ) {
        debug_assert_ne!(self.generation, 0);
        let bucket = &mut self.buckets[key as usize & (BUCKET_COUNT - 1)];
        let same = bucket
            .iter()
            .position(|entry| entry.generation == self.generation && entry.key == key);
        let slot = same.unwrap_or_else(|| {
            bucket
                .iter()
                .position(|entry| entry.generation != self.generation)
                .unwrap_or_else(|| {
                    (0..WAYS)
                        .min_by_key(|&index| (bucket[index].depth, index))
                        .expect("nonempty bucket")
                })
        });
        if same.is_some() && bucket[slot].depth > depth {
            return;
        }
        bucket[slot] = ProofEntry {
            key,
            generation: self.generation,
            depth,
            proof,
            best_move,
        };
    }
}

pub(crate) fn solver_key(key: PositionKey, attacker: Stone) -> u64 {
    key.value()
        ^ match attacker {
            Stone::Black => 0x8a5c_d789_635d_2dff,
            Stone::White => 0x121f_05b7_4299_6c13,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_keys_depth_replacement_and_public_generations_are_isolated() {
        assert_eq!(std::mem::size_of::<ProofEntry>(), 24);
        assert_eq!(std::mem::size_of::<[ProofEntry; WAYS]>(), 96);
        let mut table = ProofTable::new();
        assert_eq!(
            table.buckets.len() * std::mem::size_of::<[ProofEntry; WAYS]>(),
            393_216
        );
        table.begin_search();
        for depth in 1..=4 {
            table.store(u64::from(depth) * 4096, depth, CachedProof::NotProven, None);
        }
        assert!(table.probe(5 * 4096).is_none());
        table.store(5 * 4096, 5, CachedProof::NotProven, None);
        assert!(table.probe(4096).is_none());
        table.begin_search();
        assert!(table.probe(5 * 4096).is_none());
        table.generation = u64::MAX;
        table.begin_search();
        assert!(table.probe(2 * 4096).is_none());
        let key = PositionKey::from_position(&rustmoku_core::Position::default());
        assert_ne!(solver_key(key, Stone::Black), solver_key(key, Stone::White));
    }
}
