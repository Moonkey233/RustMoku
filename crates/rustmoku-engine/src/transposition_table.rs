use std::{mem::size_of, num::NonZeroU8, sync::atomic::Ordering};

#[cfg(loom)]
use loom::sync::atomic::AtomicU64;
#[cfg(not(loom))]
use std::sync::atomic::AtomicU64;

use rustmoku_core::Move;

const ENTRIES_PER_BUCKET: usize = 4;
const BYTES_PER_MIB: usize = 1024 * 1024;
const HASHFULL_SAMPLE_BUCKETS: usize = 1024;

/// A bounded-cost snapshot; hashfull is occupied sampled entries per thousand,
/// including entries from earlier searches. It is not a whole-table census.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TranspositionTableStatistics {
    /// Primary four-way bucket storage, excluding synchronization sidecar.
    pub capacity_bytes: usize,
    /// Version sidecar used by the bucket seqlock protocol.
    pub synchronization_bytes: usize,
    /// Primary storage plus the version sidecar. Allocator metadata is not
    /// included, just as it was not included in `capacity_bytes` previously.
    pub allocated_bytes: usize,
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

/// Logical TT data. The concurrent table stores the key separately and packs
/// every other field into one atomic word, so readers can validate one
/// key/payload snapshot under the bucket version protocol.
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

    /// Layout: signed score (32), packed move (8), depth (8), bound (8), age
    /// generation (8). The complete payload is atomic on all supported targets.
    fn encode_payload(self) -> u64 {
        let packed_move = self.best_move.map_or(0, |at| u64::from(at.0.get()));
        (self.score as u32 as u64)
            | (packed_move << 32)
            | (u64::from(self.depth) << 40)
            | (u64::from(self.bound as u8) << 48)
            | (u64::from(self.generation) << 56)
    }

    fn decode_payload(key: u64, payload: u64) -> Option<Self> {
        let bound = match ((payload >> 48) & 0xff) as u8 {
            1 => Bound::Exact,
            2 => Bound::Lower,
            3 => Bound::Upper,
            _ => return None,
        };
        let packed_move = ((payload >> 32) & 0xff) as u8;
        let best_move = if packed_move == 0 {
            None
        } else {
            Some(PackedMove(NonZeroU8::new(packed_move)?))
        };
        // Reject malformed internal payloads rather than allowing a corrupt
        // move field to become a trusted ordering/cutoff value.
        if best_move.is_some_and(|at| at.to_move().is_none()) {
            return None;
        }
        Some(Self {
            key,
            score: (payload as u32) as i32,
            best_move,
            depth: ((payload >> 40) & 0xff) as u8,
            bound,
            generation: ((payload >> 56) & 0xff) as u8,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bucket {
    entries: [TtEntry; ENTRIES_PER_BUCKET],
}

/// A 16-byte atomic slot. Four slots occupy 64 bytes; their bucket version
/// lives in a separate sidecar.
#[repr(C)]
#[derive(Debug)]
struct AtomicSlot {
    key: AtomicU64,
    payload: AtomicU64,
}

#[repr(C)]
#[derive(Debug)]
struct AtomicBucket {
    slots: [AtomicSlot; ENTRIES_PER_BUCKET],
}

impl AtomicSlot {
    fn empty() -> Self {
        Self {
            key: AtomicU64::new(0),
            payload: AtomicU64::new(0),
        }
    }
}

impl AtomicBucket {
    fn empty() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicSlot::empty()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TtStoreOutcome {
    pub(crate) stored: bool,
    pub(crate) replacement: bool,
}

/// A lock-free shared ordinary TT. Writers serialize only with other writers
/// targeting the same bucket by claiming its version word; probes never take a
/// mutex or lock. A lost store is an allowed replacement-quality race.
#[derive(Debug)]
pub(crate) struct TranspositionTable {
    buckets: Vec<AtomicBucket>,
    bucket_versions: Vec<AtomicU64>,
    mask: usize,
    replacements: AtomicU64,
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
            buckets: (0..bucket_count).map(|_| AtomicBucket::empty()).collect(),
            bucket_versions: (0..bucket_count).map(|_| AtomicU64::new(0)).collect(),
            mask: bucket_count - 1,
            replacements: AtomicU64::new(0),
        }
    }

    /// Exclusive access excludes both search workers and diagnostic readers.
    /// This is the only place versions may reset: no old snapshot can survive.
    pub(crate) fn clear(&mut self) {
        for index in 0..self.buckets.len() {
            for slot in &self.buckets[index].slots {
                slot.key.store(0, Ordering::Relaxed);
                slot.payload.store(0, Ordering::Relaxed);
            }
            self.bucket_versions[index].store(0, Ordering::Relaxed);
        }
        self.replacements.store(0, Ordering::Relaxed);
    }

    pub(crate) fn statistics(&self) -> TranspositionTableStatistics {
        let sampled = self.buckets.len().min(HASHFULL_SAMPLE_BUCKETS);
        let mut occupied = 0;
        for index in 0..sampled {
            if let Some(entries) = self.snapshot_bucket(index) {
                occupied += entries.iter().filter(|entry| entry.is_some()).count();
            }
        }
        let capacity_bytes = self.buckets.len() * size_of::<AtomicBucket>();
        let synchronization_bytes = self.bucket_versions.len() * size_of::<AtomicU64>();
        TranspositionTableStatistics {
            capacity_bytes,
            synchronization_bytes,
            allocated_bytes: capacity_bytes + synchronization_bytes,
            bucket_count: self.buckets.len(),
            entry_count: self.buckets.len() * ENTRIES_PER_BUCKET,
            hashfull_per_mille: (occupied * 1000 / (sampled * ENTRIES_PER_BUCKET)) as u16,
            replacements: self.replacements.load(Ordering::Relaxed),
        }
    }

    /// Readers validate the version around Acquire field loads. The initial
    /// Acquire of an even publication excludes older field values. If any load
    /// observes a later writer's Release field store, that writer's odd claim
    /// happens-before the final version load, which must then reject the old
    /// version. Thus an accepted snapshot belongs to one published epoch.
    /// Relaxed fields would lack the second edge; two Acquire version loads
    /// alone do not prevent mixed key/payload snapshots. Versions never wrap.
    pub(crate) fn probe(&self, key: u64) -> Option<TtEntry> {
        self.snapshot_bucket(self.bucket_index(key))?
            .into_iter()
            .flatten()
            .find(|entry| entry.key == key)
    }

    /// Returns whether the entry was stored or updated. A concurrent writer
    /// may make this a false negative; it cannot make a malformed hit visible.
    #[cfg(test)]
    pub(crate) fn store(&self, entry: TtEntry) -> bool {
        self.store_with_outcome(entry).stored
    }

    pub(crate) fn store_with_outcome(&self, entry: TtEntry) -> TtStoreOutcome {
        let bucket_index = self.bucket_index(entry.key);
        let Some(odd_version) = self.claim_bucket(bucket_index) else {
            return TtStoreOutcome::default();
        };
        let bucket = &self.buckets[bucket_index];
        let existing: [Option<TtEntry>; ENTRIES_PER_BUCKET] =
            std::array::from_fn(|index| self.load_slot(bucket, index));
        let mut stored = false;
        let mut replacement = false;

        if let Some(index) = existing
            .iter()
            .position(|candidate| candidate.is_some_and(|candidate| candidate.key == entry.key))
        {
            let current = existing[index].expect("position found an entry");
            if entry.depth < current.depth
                || (entry.depth == current.depth
                    && current.bound == Bound::Exact
                    && entry.bound != Bound::Exact)
            {
                // Refresh age even when a shallower bound is rejected.
                let refreshed = TtEntry {
                    generation: entry.generation,
                    ..current
                };
                self.write_slot(bucket, index, refreshed);
            } else {
                self.write_slot(bucket, index, entry);
                stored = true;
            }
        } else if let Some(index) = existing.iter().position(Option::is_none) {
            self.write_slot(bucket, index, entry);
            stored = true;
        } else {
            let mut replacement_index = 0;
            for index in 1..ENTRIES_PER_BUCKET {
                if replacement_priority(
                    existing[index].expect("full bucket"),
                    entry.generation,
                    index,
                ) < replacement_priority(
                    existing[replacement_index].expect("full bucket"),
                    entry.generation,
                    replacement_index,
                ) {
                    replacement_index = index;
                }
            }
            self.write_slot(bucket, replacement_index, entry);
            stored = true;
            replacement = true;
        }
        // No panics or early returns after claiming a bucket: every successful
        // claim publishes a new even version before returning.
        self.bucket_versions[bucket_index].store(odd_version + 1, Ordering::Release);
        if replacement {
            self.replacements.fetch_add(1, Ordering::Relaxed);
        }
        TtStoreOutcome {
            stored,
            replacement,
        }
    }

    fn snapshot_bucket(&self, index: usize) -> Option<[Option<TtEntry>; ENTRIES_PER_BUCKET]> {
        let version = &self.bucket_versions[index];
        for _ in 0..2 {
            let before = version.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let entries = std::array::from_fn(|slot| self.load_slot(&self.buckets[index], slot));
            let after = version.load(Ordering::Acquire);
            if before == after && after & 1 == 0 {
                return Some(entries);
            }
        }
        None
    }

    fn claim_bucket(&self, index: usize) -> Option<u64> {
        let version = &self.bucket_versions[index];
        for _ in 0..2 {
            let even = version.load(Ordering::Acquire);
            if even & 1 != 0 {
                continue;
            }
            if even >= u64::MAX - 2 {
                // Drop stores until exclusive clear/resize instead of relying
                // on how long a reader can be descheduled across version wrap.
                return None;
            }
            let odd = even + 1;
            if version
                .compare_exchange_weak(even, odd, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(odd);
            }
        }
        None
    }

    fn load_slot(&self, bucket: &AtomicBucket, index: usize) -> Option<TtEntry> {
        let slot = &bucket.slots[index];
        TtEntry::decode_payload(
            slot.key.load(Ordering::Acquire),
            slot.payload.load(Ordering::Acquire),
        )
    }

    fn write_slot(&self, bucket: &AtomicBucket, index: usize, entry: TtEntry) {
        let slot = &bucket.slots[index];
        slot.key.store(entry.key, Ordering::Release);
        slot.payload
            .store(entry.encode_payload(), Ordering::Release);
    }

    fn bucket_index(&self, key: u64) -> usize {
        (key as usize) & self.mask
    }
}

fn replacement_priority(entry: TtEntry, current_generation: u8, slot: usize) -> (i16, usize) {
    // Ages alias after 256 searches. This can change replacement quality only:
    // probes still require the full key and a sufficient depth/valid bound.
    let age = current_generation.wrapping_sub(entry.generation);
    // Four plies buy one generation; Exact entries get another generation.
    let quality =
        i16::from(entry.depth) + 4 * i16::from(entry.bound == Bound::Exact) - 4 * i16::from(age);
    (quality, slot)
}

fn floor_power_of_two(value: usize) -> usize {
    let mut power = 1;
    while power <= value / 2 {
        power *= 2;
    }
    power
}

#[cfg(all(test, loom))]
#[path = "tt_loom_tests.rs"]
mod loom_tests;

#[cfg(test)]
mod tests {
    use std::{
        mem::size_of,
        sync::{
            Arc, Barrier,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use rustmoku_core::Move;

    use super::{Bound, Bucket, ENTRIES_PER_BUCKET, PackedMove, TranspositionTable, TtEntry};

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

    fn stress_entry(writer: u64, iteration: u64) -> TtEntry {
        let key = (writer << 32) | (iteration + 1);
        TtEntry::new(
            key,
            (key as u32) as i32,
            Some(at((iteration as usize) % 225)),
            (iteration & 0xff) as u8,
            [Bound::Exact, Bound::Lower, Bound::Upper][(iteration % 3) as usize],
            writer as u8,
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
        assert_eq!(size_of::<super::AtomicSlot>(), 16);
        assert_eq!(size_of::<super::AtomicBucket>(), 64);
    }

    #[test]
    fn atomic_payload_round_trips_relevant_score_move_and_metadata_range() {
        for score in [i32::MIN, -100_000_225, -1, 0, 1, 100_000_225, i32::MAX] {
            for depth in [0, 1, 2, 127, 255] {
                for generation in [0, 1, 127, 255] {
                    for bound in [Bound::Exact, Bound::Lower, Bound::Upper] {
                        for at_move in
                            std::iter::once(None).chain((0..225).map(|index| Some(at(index))))
                        {
                            let original =
                                TtEntry::new(0xfeed_beef, score, at_move, depth, bound, generation);
                            assert_eq!(
                                TtEntry::decode_payload(original.key, original.encode_payload()),
                                Some(original)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn full_key_mismatch_in_same_bucket_does_not_hit() {
        let table = TranspositionTable::with_bucket_count(1);
        table.store(entry(1, 1, Bound::Exact, 1));
        assert!(table.probe(65).is_none());
    }

    #[test]
    fn replacement_balances_age_and_depth() {
        let table = TranspositionTable::with_bucket_count(1);
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
        let table = TranspositionTable::with_bucket_count(1);
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
        let table = TranspositionTable::with_bucket_count(1);
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
        assert_eq!(table.buckets.len() * ENTRIES_PER_BUCKET, 65_536);
        let stats = table.statistics();
        assert_eq!(stats.synchronization_bytes, table.buckets.len() * 8);
        assert_eq!(
            stats.allocated_bytes,
            stats.capacity_bytes + stats.synchronization_bytes
        );
    }

    #[test]
    fn replacement_uses_relative_age_across_wrap() {
        let table = TranspositionTable::with_bucket_count(1);
        for (key, depth, generation) in [(1, 1, 255), (2, 20, 253), (3, 1, 0), (4, 1, 254)] {
            table.store(entry(key, depth, Bound::Exact, generation));
        }
        table.store(entry(5, 1, Bound::Lower, 1));
        assert!(
            table.probe(4).is_none(),
            "old shallow entry loses to deep Exact"
        );
        for key in [1, 2, 3, 5] {
            assert!(table.probe(key).is_some());
        }
        for key in [1, 3, 5] {
            table.store(entry(key, 1, Bound::Exact, 10));
        }
        table.store(entry(6, 1, Bound::Lower, 10));
        assert!(table.probe(2).is_none());
    }

    #[test]
    fn exact_bonus_protects_an_equal_age_entry() {
        let table = TranspositionTable::with_bucket_count(1);
        table.store(entry(1, 3, Bound::Exact, 1));
        for key in 2..=4 {
            table.store(entry(key, 4, Bound::Lower, 1));
        }
        table.store(entry(5, 1, Bound::Lower, 1));
        assert!(table.probe(1).is_some());
        assert!(table.probe(2).is_none());
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
        let table = TranspositionTable::with_bucket_count(2048);
        table.store(entry(1500, 1, Bound::Exact, 1));
        assert_eq!(table.statistics().hashfull_per_mille, 0);
        for key in 0..1024 {
            table.store(entry(key, 1, Bound::Exact, 1));
        }
        assert_eq!(table.statistics().hashfull_per_mille, 250);
    }

    #[test]
    fn version_exhaustion_drops_stores_until_exclusive_clear() {
        let mut table = TranspositionTable::with_bucket_count(1);
        table.bucket_versions[0].store(u64::MAX - 3, Ordering::Relaxed);
        let last = entry(1, 3, Bound::Exact, 1);
        assert!(table.store(last));
        assert_eq!(
            table.bucket_versions[0].load(Ordering::Relaxed),
            u64::MAX - 1
        );
        assert!(!table.store(entry(1, 4, Bound::Exact, 2)));
        assert!(!table.store(entry(2, 4, Bound::Exact, 2)));
        assert_eq!(table.probe(1), Some(last));
        table.clear();
        assert!(table.probe(1).is_none());
        assert!(table.store(entry(2, 4, Bound::Exact, 2)));
    }

    #[test]
    fn concurrent_colliding_writers_never_publish_an_incoherent_hit() {
        let table = Arc::new(TranspositionTable::with_bucket_count(1));
        let start = Arc::new(Barrier::new(10));
        let mut handles = Vec::new();
        for writer in 0..8_u64 {
            let table = Arc::clone(&table);
            let start = Arc::clone(&start);
            handles.push(thread::spawn(move || {
                start.wait();
                for iteration in 0..10_000_u64 {
                    table.store(stress_entry(writer, iteration));
                }
            }));
        }
        let read_error = Arc::new(AtomicBool::new(false));
        for reader in 0..2_u64 {
            let table = Arc::clone(&table);
            let start = Arc::clone(&start);
            let read_error = Arc::clone(&read_error);
            handles.push(thread::spawn(move || {
                start.wait();
                for probe in 0..200_000_u64 {
                    let writer = (probe + reader) % 8;
                    let iteration = (probe.wrapping_mul(7) + reader) % 10_000;
                    let expected = stress_entry(writer, iteration);
                    if let Some(hit) = table.probe(expected.key)
                        && hit != expected
                    {
                        read_error.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }));
        }
        for handle in handles {
            handle.join().expect("TT stress writer must not panic");
        }
        assert!(!read_error.load(Ordering::Relaxed));
        // A probe can miss while a writer owns the bucket, but every accepted
        // hit must still equal a complete key/payload write.
        for writer in 0..8_u64 {
            for iteration in 0..10_000_u64 {
                let expected = stress_entry(writer, iteration);
                if let Some(hit) = table.probe(expected.key) {
                    assert_eq!(hit, expected);
                }
            }
        }
    }
}
