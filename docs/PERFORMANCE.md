# RustMoku Search Performance

These measurements are local relative baselines. They are useful for comparing
search revisions on this machine, but they are not cross-machine benchmarks and
must not be interpreted as engine-strength ratings. Wall-clock results naturally
vary with machine load; semantic results and node counts are more stable.


Numbers are observational. The V0.x engine is incomplete: temporary node or
wall-time regression does not automatically invalidate correctness-validated
infrastructure with concrete later uses. Verify measurements, profile, identify
the cause, and optimize it. Node count measures work searched; NPS measures a
workload-dependent throughput; wall time measures latency; TT hit rate measures
cache reuse; playing strength requires separate semantic/engine-match evidence.
These metrics are not interchangeable. Iterative deepening, TT, and PV remain.

## Environment

- CPU: AMD Ryzen 7 8845H, 8 cores / 16 logical processors
- Host: `x86_64-pc-windows-msvc`
- Toolchain: `rustc 1.98.1 (48a229cea 2026-09-01)`, LLVM 22.1.8
- Command: `cargo run --release -p rustmoku-engine --example search_bench`
- Build profile: Cargo `release` (optimized)

The benchmark uses five fixed legal Freestyle positions constructed from
deterministic move sequences in `examples/search_bench.rs`. It intentionally has
no parser, random input, Criterion dependency, or wall-clock assertion.

## V0.1 baseline

- Package version: 0.1.0
- Repository revision: `5a75ca3`
- Working-tree qualification: the benchmark harness and this document were added
  after that revision; Core and Engine search behavior were otherwise unchanged.

| Fixture | Requested depth | Best move | Score | Nodes | Elapsed (ms) | NPS |
|---|---:|---|---:|---:|---:|---:|
| opening | 4 | 96 (6,6) | 0 | 15,558 | 22.075 | 704,782 |
| balanced_midgame | 4 | 142 (9,7) | 1,960 | 19,312 | 51.083 | 378,055 |
| tactical_attack | 4 | 112 (7,7) | 99,999,999 | 6,223 | 29.777 | 208,990 |
| forced_defense | 4 | 112 (7,7) | -1,995 | 7,556 | 6.677 | 1,131,578 |
| transposition_rich | 4 | 96 (6,6) | 99,999,997 | 17,317 | 60.866 | 284,511 |

V0.1 uses a single fixed-depth Negamax search without a transposition table or
iterative deepening.

## V0.2 stateful-search comparison

- Package version: 0.2.0
- Repository revision: `08232e4` (official V0.2 commit)
- Same machine, toolchain, command, Release profile, fixtures, and requested
  depths as the V0.1 baseline

| Fixture | Requested/completed | Seldepth | Best move | Score | Nodes | TT hits | TT cutoffs | Elapsed (ms) | NPS |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|
| opening | 4/4 | 4 | 96 (6,6) | 0 | 15,257 | 4,739 | 2,932 | 19.990 | 763,220 |
| balanced_midgame | 4/4 | 4 | 142 (9,7) | 1,960 | 14,791 | 6,785 | 4,230 | 24.646 | 600,148 |
| tactical_attack | 4/4 | 4 | 107 (7,2) | 99,999,999 | 8,644 | 4,513 | 2,149 | 16.718 | 517,041 |
| forced_defense | 4/4 | 4 | 112 (7,7) | -1,995 | 6,000 | 1,203 | 829 | 8.108 | 740,028 |
| transposition_rich | 4/4 | 4 | 96 (6,6) | 99,999,997 | 12,646 | 5,623 | 1,715 | 30.278 | 417,659 |

### V0.1 versus V0.2

| Fixture | V0.1 best / score | V0.2 best / score | V0.1 nodes | V0.2 nodes | Node change | V0.1 ms | V0.2 ms |
|---|---|---|---:|---:|---:|---:|---:|
| opening | 96 / 0 | 96 / 0 | 15,558 | 15,257 | -1.9% | 22.075 | 19.990 |
| balanced_midgame | 142 / 1,960 | 142 / 1,960 | 19,312 | 14,791 | -23.4% | 51.083 | 24.646 |
| tactical_attack | 112 / 99,999,999 | 107 / 99,999,999 | 6,223 | 8,644 | +38.9% | 29.777 | 16.718 |
| forced_defense | 112 / -1,995 | 112 / -1,995 | 7,556 | 6,000 | -20.6% | 6.677 | 8.108 |
| transposition_rich | 96 / 99,999,997 | 96 / 99,999,997 | 17,317 | 12,646 | -27.0% | 60.866 | 30.278 |

The tactical fixture has two equal one-ply wins. V0.2 deliberately selects move
107, the lower canonical index; the mate score is unchanged. That semantic
tie-break correction plus iterative-deepening work explains why its node count
is higher than V0.1.

Iterative deepening repeats shallow work, while the persistent TT saves many
deeper transpositions. Winner caching and the compact allocation-free move list
also reduce hot-path work. Four fixtures visit fewer nodes, but elapsed time is
not uniformly lower: forced defense is a concrete example where iteration and
TT overhead exceed the saved work. NPS and wall time are sensitive to machine
load and cache effects; correctness, best-move/score semantics, and measured
node behavior take priority over cosmetic NPS gains.

## V0.3 measurement protocol (2026-09-05)

- V0.2 engine: official `08232e4`, with only a benchmark-driver update for these
  remeasurements; the original engine executable was captured before V0.3 edits.
- V0.3: official commit `2b41449`, version 0.3.0, based on `08232e4`.
  Same host/toolchain/Release profile as above.
- One complete untimed warm-up, then five cold searches for quick (depth 4),
  three for deep (depth 6) and capacity experiments (depth 8). The reported
  statistic is the median; an even sample count uses the upper median.
- Every timed run explicitly clears TT first. TT allocation, clearing, and
  hashfull sampling are excluded. Root state initialization and final PV
  construction are included. Every result and deterministic search statistic is
  asserted equal across cold repetitions. All suites use the five original
  fixture sequences; ordinary cargo test never runs a long benchmark.
- Runs are sequential, not randomized or pinned to a core. Desktop activity,
  clock frequency, thermals, and cache residency can affect time. Some repeated
  batches differed materially in elapsed time without any node/semantic change;
  do not infer statistical significance from a few percent of time difference.
- V0.3 defaults to PatternEvaluator; V0.2 uses ClassicalEvaluator. Scores and
  search trees can intentionally differ. This is an end-to-end version comparison,
  not a controlled experiment isolating evaluation quality or playing strength.

Reproduce V0.3:

```powershell
cargo run --release -p rustmoku-engine --example search_bench
cargo run --release -p rustmoku-engine --example search_bench -- --suite deep --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 8 --fixture opening --tt-mib 256 --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --evaluator classical
cargo run --release -p rustmoku-engine --features bench-internals --example hotpath_bench -- 100000
```

`--tt-mib` accepts 64, 256, 512, 1024 or another nonnegative integer.
`--fixture` selects one fixture and `--depth` overrides the suite's depth. All
reported searches below completed their requested depth; seldepth also equaled
that depth. CSVs contain full TT data, counts, score, best index, depth and NPS.
[Raw data and the V0.2 driver](benchmarks/README.md) are retained for reproduction.

### Quick: repeated V0.2 versus V0.3

Historical single-run tables above remain unchanged. This table uses the new
repeated protocol for both versions; best moves are zero-based board indices.

| Fixture | V0.2 best / score | V0.3 best / score | V0.2 nodes | V0.3 nodes | V0.2 median ms | V0.3 median ms | V0.3 NPS |
|---|---|---|---|---|---|---|---|
| opening | 96 / 0 | 96 / 0 | 15,257 | 7,918 | 13.503 | 4.115 | 1,924,040 |
| balanced_midgame | 142 / 1960 | 142 / 131300 | 14,791 | 14,802 | 19.034 | 7.956 | 1,860,553 |
| tactical_attack | 107 / 99999999 | 107 / 99999999 | 8,644 | 8,406 | 12.856 | 3.932 | 2,138,006 |
| forced_defense | 112 / -1995 | 112 / -243120 | 6,000 | 8,698 | 4.836 | 3.370 | 2,581,315 |
| transposition_rich | 96 / 99999997 | 96 / 99999997 | 12,646 | 11,877 | 24.169 | 5.956 | 1,994,124 |

All quick fixtures are faster in this batch (1.44x to 4.06x), including forced
defense despite more nodes. Tactical mate semantics stay unchanged. Static
scores in balanced_midgame and forced_defense change with the evaluator.

### Deep: depth 6, median of three cold runs

| Fixture | V0.2 best / score | V0.3 best / score | V0.2 nodes | V0.3 nodes | V0.2 median ms | V0.3 median ms | V0.3 NPS |
|---|---|---|---|---|---|---|---|
| opening | 129 / 0 | 96 / 0 | 5,803,248 | 788,572 | 3548.430 | 324.509 | 2,430,049 |
| balanced_midgame | 142 / 99999995 | 142 / 99999995 | 464,902 | 514,540 | 656.540 | 278.613 | 1,846,791 |
| tactical_attack | 107 / 99999999 | 107 / 99999999 | 334,654 | 337,836 | 443.220 | 232.783 | 1,451,291 |
| forced_defense | 112 / -9995 | 112 / -893960 | 425,755 | 618,990 | 319.881 | 341.776 | 1,811,101 |
| transposition_rich | 96 / 99999997 | 96 / 99999997 | 600,432 | 105,616 | 980.661 | 77.304 | 1,366,242 |

Depth 6 opening changes best index from 129 to 96, both at score 0, under the
new evaluator/tree. This is not a determinism failure across identical versions.
Each V0.3 cold repetition has identical semantic results and statistics.

The final forced_defense batch is 6.8% slower than V0.2: its nodes increase 45.4%
while NPS increases 36.1%. The new evaluation/ordering searches substantially
more nodes, partly offset by lower average node cost. Earlier V0.3 batches took
about 239 ms for the same 618,990 nodes, demonstrating timing sensitivity as
well. This is evidence for future node-reduction/strength work, not a reason to
remove TT, iterative deepening, PV, or the validated incremental state.

### ClassicalEvaluator control comparison

The explicit V0.3 ClassicalEvaluator quick and deep suites were also executed
(one warm-up, three cold repetitions). All ten `(fixture, best_move, score)`
results exactly match the corresponding captured V0.2 baseline. This control
separates new evaluator semantics from changes to state maintenance/ordering;
node counts and timings can still change. CSVs are retained as
benchmarks/v03-classical-quick.csv and benchmarks/v03-classical-deep.csv.

### Release profiling and targeted optimizations

Windows Performance Recorder CPU sampling was run on depth 6 opening, using
Release optimization plus debug symbols. xperf exported per-function samples.
Process-filtered exports are retained in benchmarks/; system-wide trace files
stay under ignored target/v03-evidence and are not repository artifacts.

The initial incremental version spent 69.2% of its process samples in
`refresh_profile` and branch-based `ThreatProfile::from_directions`. Its first
measured pattern make/unmake pair cost 1,176.45 ns. Changes were applied and
correctness-tested individually:

1. Replace branch aggregation with a 4 KiB profile lookup: 627.16 ns/pair.
2. Maintain packed per-cell directional classes and update only the changed
   direction; skip unchanged classes/profiles: 144.71 ns/pair in that batch.
3. A subsequent CPU profile attributed 33.4% of process samples to sorting.
   Pack the complete unchanged lexicographic priority into u32 and sort directly.
   Candidate generation plus ordering dropped from 891.40 to 350.31 ns.
4. Release assembly exposed a 32-byte occupancy copy in `BitBoard::test(self)`.
   Borrowing `&self` removed those stack copies; no semantic or layout change.
   The small timing difference is not treated as a statistically proven gain.

On identical depth-6 opening semantics (best 96, score 0, 788,572 nodes), observed
medians went 1,143.028 -> 847.496 -> 453.708 -> 318.364 ms through the first three
stages. The first run included WPR recording overhead, so these timings describe
observations rather than a controlled statistical speedup claim. No search
pruning, unsafe code, or third-party implementation was introduced.

Microbenchmark below uses the historical balanced_midgame position, 100,000
iterations per sample, one warm-up, five samples, and black_box on inputs/results.
A pair means one make followed by its matching unmake, cycling legal candidates.
The full initialization comparator rebuilds a state; it is not leaf evaluation.
Sub-nanosecond/small-nanosecond timings are throughput observations subject to
compiler/measurement overhead; they are not individual-call latency promises.

| Operation | Median ns |
|---|---|
| candidate_reference | 214.89 |
| candidate_incremental | 48.26 |
| frontier_make_unmake_pair | 45.56 |
| pattern_full_initialize | 4768.21 |
| pattern_make_unmake_pair | 136.67 |
| classical_evaluate | 386.69 |
| pattern_evaluate | 1.76 |
| search_state_make_unmake_pair | 198.09 |
| candidates_and_ordering | 368.99 |

Measured layout on x86_64-pc-windows-msvc: BitBoard256 32 bytes,
CandidateFrontier 296, PatternState 3,224, PatternUndo 2, and
SearchState<PatternEvaluator> 6,992. The last number includes reserved storage
for the inactive optional fallback pattern cache; no second cache is initialized
or updated for the default evaluator. Core and sidecar cell arrays remain 225
cells; a padded 256-cell experiment was not needed for the measured improvements.

### TT capacity experiment

Fixture: opening, requested/completed/seldepth 8/8/8, best index 129, score -420.
One warm-up and three cold samples per capacity. Full-key entries remain 16
bytes, four-way buckets 64 bytes. Hashfull is sampled occupied entries per
thousand (not percent and not current-generation-only).

| MiB | Nodes | Probes | Hits | Cutoffs | Replacements | Hashfull per mille | Median ms |
|---|---|---|---|---|---|---|---|
| 64 | 4,936,928 | 4,416,760 | 2,457,275 | 1,685,927 | 70,704 | 449 | 2554.721 |
| 256 | 4,936,914 | 4,416,758 | 2,467,072 | 1,686,371 | 630 | 114 | 2493.925 |
| 512 | 4,936,913 | 4,416,757 | 2,467,165 | 1,686,373 | 51 | 56 | 2560.069 |
| 1024 | 4,936,913 | 4,416,757 | 2,467,170 | 1,686,373 | 7 | 29 | 2680.330 |

Capacity is exactly the requested bytes for these four sizes. Buckets are
1,048,576 / 4,194,304 / 8,388,608 / 16,777,216; entries are four times those counts.

Larger tables sharply reduce colliding replacements, but save only 14-15 nodes
out of about 4.94 million. In the final sweep 256 MiB was 2.4% faster than 64 MiB;
an earlier sweep was 2,290 / 2,364 / 2,385 / 2,458 ms respectively, reversing that
small advantage. Neither batch establishes a robust benefit for allocating
4-16 times the memory. Default stays 64 MiB. This does not rule out larger
capacities helping substantially deeper, broader, or later mature workloads.

### Remaining hot-path work and correctness review

| Operation | V0.2 | V0.3 default |
|---|---|---|
| Candidate generation | Occupied/full board scans and radius geometry | Cached bitset iteration, ascending indices |
| Ordering | Repeated would_win and 5x5 local scans | Cached profiles, packed priorities, fixed-array sort |
| Static evaluation | Full-board contiguous-run recomputation | Nine aggregate counter differences |
| Tactical pattern recognition | Limited contiguous scoring | Build-time semantic table; bounded changed-direction lookups |
| Make/unmake | Core and hash | Core, hash, <=25 frontier counts, <=32 influenced centers |
| Root initialization | One Position clone and hash scan | One Position clone plus initial sidecar construction |
| Recursive allocation | No move-list allocations | No move-list or evaluator-state allocations |

Release assembly confirms bit scanning and mask/shift operations, no classifier
symbols in the normal library, and no division in PatternState::update_lines.
Safe bounds checks remain around 225-cell arrays and four-direction indices.
They were retained; no profiling evidence justified unsafe access. Core still
constructs checked coordinates and Results while validating moves and detecting
the new winner. That bounded rule work remains authoritative. Candidate sorting,
TT probes/stores, and PV copies remain legitimate per-node costs. ClassicalEvaluator
retains its full scans only when explicitly selected.

Review of clone/Vec/collect/unwrap/expect/unsafe/Arc/Mutex/RwLock/RefCell/allow
occurrences found the single intentional root clone, root-owned TT Vec and
final PV Vec, build-time/benchmark allocations, and test-only collections and
unwraps. Production expects cover established cache/compile-time invariants.
There is no added unsafe block, synchronization, dynamic evaluator dispatch,
per-node formatting, or suppressed lint.

The final review also fixed a V0.2 TT horizon issue exposed by Native new-game
reuse: a deeper child entry is not a bound on a shallower fixed-depth value.
Only equal-depth scores/bounds are now reusable; deeper legal TT moves still
order. The regression changed warm ancestor score 19,380 back to cold score
-580. Full-key validation, mate normalization, replacement, iterative deepening,
and PV are retained. This stricter depth rule did not change the cold benchmark
nodes or results recorded above.

### Validation actually executed

| Command | Result |
|---|---|
| cargo fmt --all -- --check | Passed |
| cargo check --workspace --all-targets | Passed |
| cargo clippy --workspace --all-targets --all-features -- -D warnings | Passed |
| cargo test --workspace --all-features | Passed: 86 tests (14 Core, 57 Engine unit, 15 Engine integration) |
| cargo test --release -p rustmoku-engine | Passed: 72 tests |
| cargo build --release -p rustmoku-native | Passed |
| cargo run --release -p rustmoku-engine --example search_bench | Passed: quick suite, default arguments |
| deep suite and 64/256/512/1024 MiB depth-8 searches | Passed, run explicitly outside cargo test |
| hotpath_bench with bench-internals | Passed, run explicitly outside cargo test |
| cargo rustc --release -p rustmoku-engine --lib -- --emit=asm | Passed; inspected optimized output |

Native Windows smoke testing covered launch, Black move and AI reply, occupied
cell rejection, side switching to White with AI center opening, New Game, depth/
score/TT/PV display, and the corrected cross-position warm-cache result (-580).
The GUI remains synchronous. These checks are not an arena or strength test.

## V0.3 → V0.4 search core summary (2026-09-05)

Baseline: official `2b41449` (0.3.0); upgraded workspace: 0.4.0. Same Windows x64
host, toolchain, Release profile, PatternEvaluator, and 64 MiB TT. Each fixture
used one untimed warm-up and three measured cold searches; repeated results and
all statistics matched exactly. Quick uses nominal depth 4; the two representative
longer searches use depth 6. Times below are medians; aggregate sums fixture medians.

| Workload | Depth | V0.3 best/score | V0.4 best/score | Nodes V0.3 → V0.4 (qnodes) | ms V0.3 → V0.4 | Time change |
|---|---:|---|---|---|---|---:|
| quick aggregate (5 fixtures) | 4 | — | — | 51,701 → 184,493 (168,803) | 23.405 → 68.931 | +194.5% |
| quick balanced_midgame | 4 | 142/131300 | 142/99999995 | 14,802 → 88,646 (84,690) | 7.413 → 32.569 | +339.3% |
| opening | 6 | 96/0 | 129/18340 | 788,572 → 605,631 (501,198) | 320.897 → 224.315 | -30.1% |
| forced_defense | 6 | 112/-893960 | 112/-20200 | 618,990 → 664,877 (618,864) | 235.576 → 201.265 | -14.6% |

The quick increase is explained by the new threat horizon: 168,803 of 184,493
nodes are qnodes, with seldepth up to 10 instead of 4. Balanced midgame now sees
mate-distance 5 at nominal depth 4. Aggregate node cost decreases while the extra
threat work raises elapsed time; this is not an unexplained per-node regression.
The depth-6 searches reach seldepth 12. Opening records 501,198 qnodes, 34 PVS/tie
re-searches, and one aspiration failure in each direction; forced_defense records
618,864 qnodes, 68 re-searches, and one fail-low. Best moves/scores may differ
between versions because qsearch deliberately changes the horizon. These timings
and small tactical regressions are not engine-match evidence of overall strength.

SearchState<PatternEvaluator> is 3,768 bytes versus 6,992 in V0.3; exactly one
PatternState remains and both current evaluators have unit State/Undo. TT entries
remain 16 bytes and four-way buckets 64 bytes. No capacity experiment, new
microbenchmark, profiler, assembly inspection, or historical Classical suite was
run for this milestone. Reproduce with the three V0.4 commands in README.md.

Final validation passed: fmt check, workspace/all-targets check, all-feature
Clippy with warnings denied, all-feature workspace tests (94: 14 Core + 65 Engine
unit + 15 integration), Release Engine tests (80), and Release Native build.
