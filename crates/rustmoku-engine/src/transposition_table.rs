use std::{mem::size_of, num::NonZeroU8};

use rustmoku_core::Move;

const ENTRIES_PER_BUCKET: usize = 4;
const BYTES_PER_MIB: usize = 1024 * 1024;
const HASHFULL_SAMPLE_BUCKETS: usize = 1024;

/// A bounded-cost snapshot; hashfull is occupied sampled entries per thousand,
/// including entries from earlier searches. It is not a whole-table census.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TranspositionTableStatistics {
    pub capacity_bytes: usize,
    pub bucket_count: usize,
    pub entry_count: usize,
    pub hashfull_per_mille: u16,
    /// Colliding full-key evictions since the last explicit clear or resize.
    pub replacements: u64,
}

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
    replacements: u64,
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
            replacements: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.buckets.fill(Bucket::default());
        self.replacements = 0;
    }

    pub(crate) fn statistics(&self) -> TranspositionTableStatistics {
        let sampled = self.buckets.len().min(HASHFULL_SAMPLE_BUCKETS);
        let occupied = self.buckets[..sampled]
            .iter()
            .flat_map(|bucket| &bucket.entries)
            .filter(|entry| !entry.is_empty())
            .count();
        TranspositionTableStatistics {
            capacity_bytes: self.buckets.len() * size_of::<Bucket>(),
            bucket_count: self.buckets.len(),
            entry_count: self.buckets.len() * ENTRIES_PER_BUCKET,
            hashfull_per_mille: (occupied * 1000 / (sampled * ENTRIES_PER_BUCKET)) as u16,
            replacements: self.replacements,
        }
    }

    pub(crate) const fn replacements(&self) -> u64 {
        self.replacements
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
        self.replacements += 1;
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
    // Ages alias after 256 searches. This can change replacement quality only:
    // probes still require the full key and a sufficient depth/valid bound.
    let age = current_generation.wrapping_sub(entry.generation);
    (u8::MAX - age, entry.depth, slot)
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

    #[test]
    fn replacement_uses_relative_age_across_wrap() {
        let mut table = TranspositionTable::with_bucket_count(1);
        for (key, depth, generation) in [(1, 1, 255), (2, 20, 253), (3, 1, 0), (4, 1, 254)] {
            table.store(entry(key, depth, Bound::Exact, generation));
        }
        table.store(entry(5, 1, Bound::Lower, 1));
        assert!(
            table.probe(2).is_none(),
            "age 4 precedes ages 1, 2, 3, even at greater depth"
        );
        for key in [1, 3, 4, 5] {
            assert!(table.probe(key).is_some());
        }
    }

    #[test]
    fn statistics_count_only_colliding_evictions_and_clear_resets_them() {
        let mut table = TranspositionTable::with_bucket_count(1);
        assert_eq!(table.statistics().hashfull_per_mille, 0);
        for key in 1..=4 {
            table.store(entry(key, 3, Bound::Exact, 1));
        }
        assert_eq!(table.statistics().hashfull_per_mille, 1000);
        assert_eq!(table.statistics().replacements, 0);
        table.store(entry(1, 4, Bound::Exact, 2));
        table.store(entry(1, 1, Bound::Upper, 2));
        assert_eq!(table.statistics().replacements, 0);
        table.store(entry(5, 4, Bound::Exact, 2));
        assert_eq!(table.statistics().replacements, 1);
        table.clear();
        let stats = table.statistics();
        assert_eq!(stats.hashfull_per_mille, 0);
        assert_eq!(stats.replacements, 0);
        assert_eq!(stats.capacity_bytes, 64);
        assert_eq!(stats.entry_count, 4);
    }

    #[test]
    fn sampling_is_bounded_and_deterministic() {
        let mut table = TranspositionTable::with_bucket_count(2048);
        table.store(entry(1500, 1, Bound::Exact, 1));
        assert_eq!(table.statistics().hashfull_per_mille, 0);
        for key in 0..1024 {
            table.store(entry(key, 1, Bound::Exact, 1));
        }
        assert_eq!(table.statistics().hashfull_per_mille, 250);
    }
}
