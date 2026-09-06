//! Model the actual TT implementation, including writer claims and snapshots.
//! Run with RUSTFLAGS="--cfg loom" cargo test --release -p rustmoku-engine --lib tt_loom.
use loom::{sync::Arc, thread};

use super::{Bound, TranspositionTable, TtEntry};

fn entry(key: u64, score: i32) -> TtEntry {
    TtEntry::new(key, score, None, 4, Bound::Exact, 1)
}

#[test]
fn tt_loom_competing_writers_preserve_complete_entries() {
    for keys in [[1, 1], [5, 6]] {
        // Bound preemptions for the three active threads; the single-writer
        // regression exhausts its interleavings without this bound.
        let mut model = loom::model::Builder::new();
        model.preemption_bound = Some(2);
        model.check(move || {
            let table = Arc::new(TranspositionTable::with_bucket_count(1));
            for key in 1..=4 {
                assert!(table.store(entry(key, key as i32)));
            }
            let first = entry(keys[0], 50_000);
            let second = entry(keys[1], -60_000);
            let first_table = Arc::clone(&table);
            let first_writer = thread::spawn(move || {
                first_table.store(first);
            });
            let second_table = Arc::clone(&table);
            let second_writer = thread::spawn(move || {
                second_table.store(second);
            });
            let reader = thread::spawn(move || {
                if let Some(entries) = table.snapshot_bucket(0) {
                    for hit in entries.into_iter().flatten() {
                        assert!(
                            hit == first
                                || hit == second
                                || ((1..=4).contains(&hit.key)
                                    && hit == entry(hit.key, hit.key as i32)),
                            "mixed TT publication: {hit:?}"
                        );
                    }
                }
            });
            first_writer.join().unwrap();
            second_writer.join().unwrap();
            reader.join().unwrap();
        });
    }
}

#[test]
fn tt_loom_reader_rejects_mixed_replacement() {
    loom::model(|| {
        let table = Arc::new(TranspositionTable::with_bucket_count(1));
        // Fill the bucket so the writer replaces key 1, rather than an empty slot.
        for key in 1..=4 {
            assert!(table.store(entry(key, key as i32)));
        }
        let writer_table = Arc::clone(&table);
        let writer = thread::spawn(move || {
            assert!(writer_table.store(entry(5, 50_000)));
        });
        let reader = thread::spawn(move || {
            if let Some(entries) = table.snapshot_bucket(0) {
                for hit in entries.into_iter().flatten() {
                    let expected = if hit.key == 5 {
                        entry(5, 50_000)
                    } else {
                        entry(hit.key, hit.key as i32)
                    };
                    assert_eq!(hit, expected, "mixed TT publication");
                }
            }
        });
        writer.join().unwrap();
        reader.join().unwrap();
    });
}
