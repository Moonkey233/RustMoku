use std::{mem::size_of, num::NonZeroU8};

use rustmoku_core::Move;

const ENTRIES_PER_BUCKET: usize = 4;
const BYTES_PER_MIB: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Bound {
    #[default]
    Empty,
    Exact,
    Lower,
    Upper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
struct PackedMove(NonZeroU8);

impl PackedMove {
    fn from_move(at: Move) -> Option<Self> {
        // Valid move indices are 0..=224, so index + 1 is non-zero and fits u8.
        u8::try_from(at.index() + 1)
            .ok()
            .and_then(NonZeroU8::new)
            .map(Self)
    }

    fn to_move(self) -> Option<Move> {
        Move::from_index(usize::from(self.0.get() - 1)).ok()
    }
}

/// Field order intentionally yields a 16-byte entry on supported targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TtEntry {
    pub(crate) key: u64,
    pub(crate) score: i32,
    best_move: Option<PackedMove>,
    pub(crate) depth: u8,
    pub(crate) bound: Bound,
    pub(crate) generation: u8,
}

impl TtEntry {
    pub(crate) fn new(
        key: u64,
        score: i32,
        best_move: Option<Move>,
        depth: u8,
        bound: Bound,
        generation: u8,
    ) -> Self {
        debug_assert!(bound != Bound::Empty);
        Self {
            key,
            score,
            best_move: best_move.and_then(PackedMove::from_move),
            depth,
            bound,
            generation,
        }
    }

    pub(crate) fn best_move(self) -> Option<Move> {
        self.best_move.and_then(PackedMove::to_move)
    }

    const fn is_empty(self) -> bool {
        matches!(self.bound, Bound::Empty)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bucket {
    entries: [TtEntry; ENTRIES_PER_BUCKET],
}

#[derive(Debug)]
pub(crate) struct TranspositionTable {
    buckets: Vec<Bucket>,
    mask: usize,
}

impl TranspositionTable {
    pub(crate) fn new(memory_mib: usize) -> Self {
        let requested_bytes = memory_mib.saturating_mul(BYTES_PER_MIB);
        let raw_bucket_count = (requested_bytes / size_of::<Bucket>()).max(1);
        Self::with_bucket_count(floor_power_of_two(raw_bucket_count))
    }

    fn with_bucket_count(bucket_count: usize) -> Self {
        debug_assert!(bucket_count.is_power_of_two());
        Self {
            buckets: vec![Bucket::default(); bucket_count],
            mask: bucket_count - 1,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.buckets.fill(Bucket::default());
    }

    pub(crate) fn probe(&self, key: u64) -> Option<TtEntry> {
        self.bucket(key)
            .entries
            .iter()
            .copied()
            .find(|entry| !entry.is_empty() && entry.key == key)
    }

    pub(crate) fn store(&mut self, entry: TtEntry) -> bool {
        let bucket = self.bucket_mut(entry.key);

        if let Some(existing) = bucket
            .entries
            .iter_mut()
            .find(|existing| !existing.is_empty() && existing.key == entry.key)
        {
            if entry.depth < existing.depth
                || (entry.depth == existing.depth
                    && existing.bound == Bound::Exact
                    && entry.bound != Bound::Exact)
            {
                existing.generation = entry.generation;
                return false;
            }
            *existing = entry;
            return true;
        }

        if let Some(empty) = bucket
            .entries
            .iter_mut()
            .find(|existing| existing.is_empty())
        {
            *empty = entry;
            return true;
        }

        let mut replacement = 0;
        for candidate in 1..ENTRIES_PER_BUCKET {
            if replacement_priority(bucket.entries[candidate], entry.generation, candidate)
                < replacement_priority(bucket.entries[replacement], entry.generation, replacement)
            {
                replacement = candidate;
            }
        }
        bucket.entries[replacement] = entry;
        true
    }

    fn bucket(&self, key: u64) -> &Bucket {
        &self.buckets[self.bucket_index(key)]
    }

    fn bucket_mut(&mut self, key: u64) -> &mut Bucket {
        let index = self.bucket_index(key);
        &mut self.buckets[index]
    }

    fn bucket_index(&self, key: u64) -> usize {
        (key as usize) & self.mask
    }
}

fn replacement_priority(entry: TtEntry, current_generation: u8, slot: usize) -> (u8, u8, usize) {
    let is_current = u8::from(entry.generation == current_generation);
    (is_current, entry.depth, slot)
}

fn floor_power_of_two(value: usize) -> usize {
    let mut power = 1;
    while power <= value / 2 {
        power *= 2;
    }
    power
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use rustmoku_core::Move;

    use super::{Bound, Bucket, PackedMove, TranspositionTable, TtEntry};

    fn at(index: usize) -> Move {
        Move::from_index(index).expect("test move index must be valid")
    }

    fn entry(key: u64, depth: u8, bound: Bound, generation: u8) -> TtEntry {
        TtEntry::new(
            key,
            i32::from(depth),
            Some(at(depth as usize)),
            depth,
            bound,
            generation,
        )
    }

    #[test]
    fn packed_move_uses_option_niche_and_round_trips() {
        assert_eq!(size_of::<PackedMove>(), 1);
        assert_eq!(size_of::<Option<PackedMove>>(), 1);
        for index in 0..225 {
            let packed = PackedMove::from_move(at(index));
            assert_eq!(packed.and_then(PackedMove::to_move), Some(at(index)));
        }
    }

    #[test]
    fn entry_and_bucket_have_cache_local_layout() {
        assert_eq!(size_of::<TtEntry>(), 16);
        assert_eq!(size_of::<Bucket>(), 64);
    }

    #[test]
    fn full_key_mismatch_in_same_bucket_does_not_hit() {
        let mut table = TranspositionTable::with_bucket_count(1);
        table.store(entry(1, 1, Bound::Exact, 1));
        assert!(table.probe(65).is_none());
    }

    #[test]
    fn replacement_prefers_old_then_shallow_entries() {
        let mut table = TranspositionTable::with_bucket_count(1);
        for (key, depth, generation) in [(1, 8, 2), (2, 2, 1), (3, 5, 1), (4, 1, 2)] {
            table.store(entry(key, depth, Bound::Lower, generation));
        }

        table.store(entry(5, 4, Bound::Exact, 2));
        assert!(
            table.probe(2).is_none(),
            "shallow old entry should be replaced"
        );
        assert!(table.probe(1).is_some());
        assert!(table.probe(3).is_some());
        assert!(table.probe(4).is_some());
        assert!(table.probe(5).is_some());
    }

    #[test]
    fn current_generation_is_preferred_over_old_generation() {
        let mut table = TranspositionTable::with_bucket_count(1);
        for key in 1..=4 {
            table.store(entry(key, 4, Bound::Lower, 1));
        }
        table.store(entry(1, 4, Bound::Lower, 2));
        table.store(entry(5, 4, Bound::Lower, 2));

        assert!(table.probe(1).is_some());
        assert!(table.probe(2).is_none());
    }

    #[test]
    fn shallow_bound_does_not_destroy_deeper_exact_same_key() {
        let mut table = TranspositionTable::with_bucket_count(1);
        table.store(entry(7, 8, Bound::Exact, 1));
        assert!(!table.store(entry(7, 2, Bound::Lower, 2)));

        let stored = table.probe(7).expect("entry must remain present");
        assert_eq!(stored.depth, 8);
        assert_eq!(stored.bound, Bound::Exact);
        assert_eq!(stored.generation, 2);
    }

    #[test]
    fn clear_empties_all_entries() {
        let mut table = TranspositionTable::with_bucket_count(2);
        table.store(entry(1, 1, Bound::Exact, 1));
        table.store(entry(2, 1, Bound::Exact, 1));
        table.clear();
        assert!(table.probe(1).is_none());
        assert!(table.probe(2).is_none());
    }

    #[test]
    fn capacity_uses_power_of_two_bucket_count() {
        let table = TranspositionTable::new(1);
        assert_eq!(table.buckets.len() * size_of::<Bucket>(), 1024 * 1024);
        assert_eq!(table.buckets.len() * 4, 65_536);
    }
}
