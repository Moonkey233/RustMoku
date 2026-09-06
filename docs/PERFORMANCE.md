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

Baseline: official `2b41449` (0.3.0); V0.4: official `882a82d` (0.4.0). Same Windows x64
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

## V0.4 → V0.5 tactical selectivity (2026-09-05)

Baseline `882a82d` (0.4.0), upgraded version 0.5.0; same host, Release profile,
PatternEvaluator and 64 MiB TT. One untimed warm-up then three cold runs per
fixture, with identical semantic results/statistics on all repetitions. Times
are medians; quick aggregate sums the five fixture medians. LMR counts are V0.5
reductions/full-depth retries (V0.4 has neither).

| Workload (depth) | Best/score V0.4 → V0.5 | Nodes V0.4 → V0.5 | Qnodes V0.4 → V0.5 | LMR/retries | ms V0.4 → V0.5 |
|---|---|---:|---:|---:|---:|
| opening (4) | 129/1120 → 96/780 | 27,555 → 8,977 | 24,901 → 4,673 | 0/0 | 9.064 → 3.687 |
| balanced_midgame (4) | 142/99999995 → 142/99999995 | 88,646 → 16,080 | 84,690 → 12,122 | 0/0 | 31.258 → 7.501 |
| tactical_attack (4) | 107/99999999 → 107/99999999 | 16,528 → 4 | 13,162 → 0 | 0/0 | 6.194 → 0.008 |
| forced_defense (4) | 112/-3660 → 112/-243120 | 10,569 → 7,406 | 9,559 → 6,260 | 0/0 | 3.149 → 2.360 |
| transposition_rich (4) | 96/99999997 → 96/99999997 | 41,195 → 1,772 | 36,491 → 1,190 | 0/0 | 15.334 → 0.937 |
| opening (6) | 129/18340 → 129/19180 | 605,631 → 147,870 | 501,198 → 87,463 | 11,228/131 | 221.560 → 67.141 |
| forced_defense (6) | 112/-20200 → 112/-243380 | 664,877 → 222,413 | 618,864 → 161,115 | 4,317/134 | 194.715 → 95.678 |
| **Quick aggregate (4)** | — | **184,493 → 34,239** | **168,803 → 24,245** | 0/0 | **64.999 → 14.493** |

Quick elapsed decreases 77.7%, depth-6 opening 69.7%, and forced_defense 50.9%.
Normal nodes (nodes minus qnodes) change from 15,690 to 9,994 in quick, 104,433 to
60,407 in opening, and 46,013 to 61,298 in forced_defense. The latter grows 33.2%
after changed leaf semantics/order and conservative TT storage, while its qnodes
fall 74.0% and total time halves. LMR is intentionally inactive on these shallow
quick searches; exact prechecks and narrower corrected qsearch explain their
savings. No isolated LMR speedup or global playing-strength gain is inferred.

Winning fixtures retain mate-in-one, mate-in-three, and mate-in-five scores;
forced_defense retains its required block. Non-mate scores and the
quick opening choice change with qsearch semantics. Warm/cold root/PV regressions
also cover both depth-6 fixtures. Direct immediate facts are exact; LMR fail-low
remains heuristic, with no nominal-depth TT store from a reduced subtree. No
profiler or additional benchmark matrix was needed. Reproduce using the same
three commands in README.md.

Final validation passed: fmt check, workspace/all-targets check, all-feature
Clippy with warnings denied, workspace tests (102: 14 Core + 73 Engine unit + 15
integration), Release Engine tests (88), and Release Native build. Eight focused
regressions were added; existing lifecycle tests now also verify profile bitsets.


## V0.6 exact VCF lean check (2026-09-05)

Baseline: official V0.5 `faf36748`, measured before edits with its saved Release
benchmark executable. V0.6 uses the same host/toolchain, PatternEvaluator,
64 MiB ordinary TT, untimed warm-up and cold TT clearing. Quick and the two new
fixtures use five repetitions; depth-six cases use three. Times are medians;
quick aggregate is the sum of its five medians. Proof-table allocation is
untimed; generation advance and root state construction are timed. New public
searches logically invalidate proof cache without clearing its allocation.

Default VCF limits: 11 proof plies / 2,000 nodes; proof table: 384 KiB. The
existing quick suite is unchanged. No historical V0.1-V0.4, capacity, hot-path,
assembly, exhaustive-pattern benchmark, or match/strength study was rerun.

| Workload | Nodes / qnodes | VCF nodes / hits / proofs | Best index / score | V0.5 ms | V0.6 ms |
|---|---:|---:|---|---:|---:|
| Quick (five depth-4 fixtures) | 32,462 / 23,054 | 21 / 0 / 2 | All unchanged; details below | 14.584 | 14.519 |
| opening (depth 6) | 123,374 / 75,116 | 0 / 0 / 0 | 129 / 19180 | 67.846 | 56.152 |
| forced_defense (depth 6) | 189,850 / 140,800 | 0 / 0 / 0 | 112 / -243380 | 99.407 | 87.425 |
| vcf_win (depth 4) | 0 / 0 | 35 / 0 / 1 | 111 / 99999995 | - | 0.025 |
| non_vcf_tactical (depth 4) | 27,912 / 21,841 | 54 / 0 / 0 | 94 / 22040 | - | 10.033 |

Quick details (V0.6); all five best moves/scores match V0.5:

| Fixture | Nodes / qnodes | VCF nodes / hits / proofs | Best / score | ms |
|---|---:|---:|---|---:|
| opening | 8976 / 4672 | 0 / 0 / 0 | 96 / 780 | 3.999 |
| balanced_midgame | 16080 / 12122 | 0 / 0 / 0 | 142 / 99999995 | 7.774 |
| tactical_attack | 0 / 0 | 2 / 0 / 1 | 107 / 99999999 | 0.015 |
| forced_defense | 7406 / 6260 | 0 / 0 / 0 | 112 / -243120 | 2.709 |
| transposition_rich | 0 / 0 | 19 / 0 / 1 | 96 / 99999997 | 0.022 |

The quick aggregate is effectively unchanged (-0.4%). Depth-six opening takes
17.2% less time and forced_defense 12.1% less. Their best move/score remain
unchanged; nodes fall from 147,870 to 123,374 and 222,413 to 189,850. This is an
end-to-end CR/state/VCF change, not a claim that all gains come from VCF. Quiet
opening, balanced_midgame, and forced_defense roots spend zero VCF nodes/probes.
Depth-four forced_defense timing rises 14.8% in this batch with identical node
counts and no solver invocation; no quiet case exceeds the 15-20% investigation
threshold. Sub-millisecond proof timings are observational, not strength evidence.

`vcf_win` starts from indices `[108,107,109,0,110,2,66,4,81,6]`. Its proof PV is
`[111,112,96,1,51]`: a forced horizontal reply followed by an open vertical four.
It returns score 99,999,995, completed nominal depth 0, and proof/seldepth 5.
`non_vcf_tactical` starts from `[108,107,109,0,110,2]`, a one-ended three. Its
Four+ gate runs, but 54 proof nodes find no VCF within 11 plies; ordinary search
continues and completes depth 4. All measured proof attempts stay within budget.
Cache hits are zero on these small first-attempt fixtures; focused tests exercise
same-public-search cache reuse, certificate loss, and cold/warm history isolation.

Alpha-Beta nodes/qnodes exclude VCF; VCF nodes count visits including depth-zero
nodes and cache hits. Certificate replay is bounded, nonbranching reconstruction
and does not spend expansion nodes. `vcf_probes` counts solver attempts. Raw CSVs
retain all normal/VCF statistics, completed depth, seldepth, and timings:
[V0.5 lean](benchmarks/v05-lean.csv), [V0.6 lean](benchmarks/v06-lean.csv).

Reproduce the five workloads:

```powershell
cargo run --release -p rustmoku-engine --example search_bench -- --suite quick --repeats 5
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture opening --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture forced_defense --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --fixture vcf_win --repeats 5
cargo run --release -p rustmoku-engine --example search_bench -- --fixture non_vcf_tactical --repeats 5
```


## V0.7: exact VCT / threat-space / DFPN (2026-09-05)

Baseline is the clean official V0.6 commit
`aff61e4ba303e9145e87b7ca32832d6d82b64886`, measured in this session before edits.
Both revisions use Release, Rust 1.98.1, PatternEvaluator, 64 MiB ordinary TT,
one untimed warm-up, then three cold runs on the same Windows host. Times are
medians; quick aggregate sums the five fixture medians. Table allocation and
clearing are outside timing. Each timed run asserts identical full results and
statistics. No historical matrix, arena, WPR, assembly, or NNUE work was rerun.

V0.7 defaults: VCF 11 plies / 2,000 nodes; VCT 9 plies / 4,000 inspections;
16 MiB VCT table request, 12 MiB allocated (262,144 x 48-byte entries).
Separate embedded tactical metadata is 512 KiB; hot patterns remain 128 KiB.

| Workload | Nominal / completed / seldepth | Best / score | AB nodes / qnodes | VCF nodes | VCT nodes / hits / proven / exhausted | V0.6 ms | V0.7 ms |
|---|---|---|---:|---:|---:|---:|---:|
| Quick aggregate (five fixtures) | 4 / varies / varies | All best/score unchanged | 16,383 / 10,932 | 11 | 805 / 32 / 1 / 0 | 13.604 | 6.701 |
| opening | 6 / 6 / 10 | 129 / 19,180 | 123,374 / 75,116 | 0 | 330 / 13 / 0 / 0 | 52.231 | 52.195 |
| forced_defense | 6 / 6 / 10 | 112 / -243,380 | 189,211 / 140,782 | 0 | 0 / 0 / 0 / 0 | 84.013 | 81.908 |
| vcf_win | 4 / 0 / 5 | 111 / 99,999,995 | 0 / 0 | 19 | 0 / 0 / 0 / 0 | 0.015 | 0.013 |
| vct_win | 4 / 0 / 5 | 112 / 99,999,995 | 0 / 0 | 0 | 915 / 31 / 1 / 0 | n/a | 0.242 |
| non_vct_tactical | 4 / 4 / 6 | 95 / 21,120 | 15,397 / 11,031 | 0 | 330 / 13 / 0 / 0 | n/a | 5.403 |

Quick improves 50.7% because balanced_midgame now returns a complete five-ply VCT
proof, preserving its best move/score while replacing 16,080 Alpha-Beta nodes.
Opening depth six is effectively unchanged (-0.1%); forced defense improves 2.5%
with 639 fewer nodes. No ordinary quiet workload regresses by 15-20%, so profiling
was unnecessary. Sub-millisecond proof timings are noisy; node counts are the
reproducible evidence. VCF parity removes impossible-depth work: vcf_win visits
19 instead of 35 nodes. No VCF table micro-optimization was performed.

| Quick fixture | Best / score | Completed / seldepth | V0.6 ms | V0.7 ms |
|---|---|---|---:|---:|
| opening | 96 / 780 | 4 / 7 | 3.701 | 3.943 |
| balanced_midgame | 142 / 99,999,995 | 0 / 5 | 7.447 | 0.123 |
| tactical_attack | 107 / 99,999,999 | 0 / 1 | 0.013 | 0.009 |
| forced_defense | 112 / -243,120 | 4 / 8 | 2.421 | 2.616 |
| transposition_rich | 96 / 99,999,997 | 0 / 3 | 0.022 | 0.010 |

The new proven fixture uses indices `[110,0,111,14,82,210,97,224]`.
Move 112 makes a DoubleThree; every relevant defense loses, and the canonical
PV legally reaches a Black win in five plies. The non-proven fixture is
`[110,0,111,224]`: OpenThree candidates exist but a direct defense defeats each
bounded attempt. It returns NoProof and completes normal depth four.
Empty/no-OpenThree roots spend zero VCT nodes. The opening fixture does have
OpenThree candidates; its complete bounded refutation costs 330 inspections.

AB nodes/qnodes exclude both tactical solvers. VCT inspections include child
initialization, cache hits, and canonical reconstruction work under the same
budget. Only fixed immediate prefixes and final PV copying are uncharged.
VCF retains its bounded, uncharged nonbranching replay. Counts are not claims
about total CPU operations. Warm public proof caches cannot change budget
outcomes because each public search advances both tactical generations.

Reproduction (the six workloads executed for this milestone):

```powershell
cargo run --release -p rustmoku-engine --example search_bench -- --suite quick --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture opening --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture forced_defense --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --fixture vcf_win --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --fixture vct_win --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --fixture non_vct_tactical --repeats 3
```


Validation completed successfully from the workspace root:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed, no warnings |
| `cargo test --workspace --all-features` | Passed: 128 tests (14 Core, 99 engine unit, 15 engine integration) |
| `cargo test --release -p rustmoku-engine` | Passed: 114 tests |
| `cargo build --release -p rustmoku-native` | Passed |

The 14 new focused tests cover directional LMR/TT evidence, VCF parity and wider
proof-cache reuse, exhaustive metadata generation, response omission audits,
external Four counter-threats, AND refutations, threat-context isolation,
DFPN thresholds, interruption during search/certification, a shallow all-legal
minimax oracle, min/max distance, canonical ties, terminal PV replay, exact board
restoration, and public root integration. Existing tests remain in the suite.


## V0.8 lifecycle / Arena / async Native lean check (2026-09-06)

The baseline is official V0.7 `d1d97088ea418b80df1ba7759958f30dc64aef53`.
Its Release benchmark executable was saved before implementation and the four
baseline workloads were measured then. Work resumed from the preserved V0.8
working tree without repair or reset. Because host timing changed across the
interruption, the saved unmodified V0.7 executable and final V0.8 executable were
measured consecutively again on the same host for each workload below. These
paired measurements, rather than historical timings, are the comparison basis.

Both use the repository Rust toolchain, Release, PatternEvaluator, a 64 MiB TT,
default proof settings, one untimed warm-up and seven cold repetitions. Allocation
and TT clearing are outside timing. No time/node limit is active. Times are
medians; quick sums its five medians. Sub-millisecond proof times remain noisy.
No historical matrix, TT sweep, assembly inspection, WPR, or large match was run.

| Workload | Requested / completed / seldepth | Best index / score | AB nodes / qnodes | VCF / VCT nodes | Total work | V0.7 ms | V0.8 ms | Time change |
|---|---|---|---:|---:|---:|---:|---:|---:|
| Quick aggregate (five fixtures) | 4 / varies / varies | All unchanged | 16,383 / 10,932 | 11 / 805 | 17,199 | 14.675 | 15.196 | +3.6% |
| opening | 6 / 6 / 10 | 129 / 19,180 | 123,374 / 75,116 | 0 / 330 | 123,704 | 112.330 | 111.623 | -0.6% |
| forced_defense | 6 / 6 / 10 | 112 / -243,380 | 189,211 / 140,782 | 0 / 0 | 189,211 | 175.192 | 176.776 | +0.9% |
| vct_win | 4 / 0 / 5 | 112 / 99,999,995 | 0 / 0 | 0 / 915 | 915 | 0.530 | 0.525 | -0.9% |

All common deterministic statistics, best moves and scores are unchanged in
these workloads. Quick best/score pairs are opening 96/780, balanced_midgame
142/99,999,995, tactical_attack 107/99,999,999, forced_defense 112/-243,120,
and transposition_rich 96/99,999,997. Depth-six AB NPS is 1,105,270 for opening
and 1,070,346 for forced_defense. No ordinary workload exceeds the 5–10%
investigation threshold; the fixed 256-work-node atomic/clock poll stride needs
no further tuning. This is overhead evidence, not a strength claim.

`search_bench` retains existing subsystem counters and AB NPS and now also emits
`work_nodes` and `termination`. Work includes AB nodes (qnodes counted once), VCF
visits/replay and VCT inspections/certificates. Local proof caps are independent.
Exact-known-loss PV leaves intentionally change from an unrelated lowest empty
cell to the first opponent winning point, followed by the next winning point;
mate score/distance are preserved. The all-legal VCT oracle follows that explicit
resistance policy and continues comparing complete bounded certificates.

Reproduction of the only four performance workloads:

```powershell
cargo run --release -p rustmoku-engine --example search_bench -- --suite quick --repeats 7
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture opening --repeats 7
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture forced_defense --repeats 7
cargo run --release -p rustmoku-engine --example search_bench -- --fixture vct_win --repeats 7
```

### Arena smoke only

```powershell
cargo run --release -p rustmoku-arena -- --pairs 2 --depth 2 --nodes 2000 --b-vct-nodes 0
```

A uses default configuration; B disables only VCT. The shared suite's first two
openings are `diagonal` (H8 H9 I7 I8) and `cross` (H8 I8 H7 I9). Each uses the
identical legal prefix for both color-swapped legs; each game has fresh engines
with ordinary TT state retained between its moves.

| Opening | A color | Winner | Total plies | Searched moves | Work |
|---|---|---|---:|---:|---:|
| diagonal | Black | B | 22 | 18 | 19,068 |
| diagonal | White | B | 33 | 29 | 34,261 |
| cross | Black | A | 41 | 37 | 41,391 |
| cross | White | B | 23 | 19 | 8,971 |

A wins 1, B wins 3, draws 0. A scores 1.0/4 points (0.5 points per pair).
The 103 searched moves consume 103,691 work nodes, averaging 1,006.7 per move.
This verifies legal completion, configuration, paired colors, accounting and CSV
output only. It does not establish superiority, Elo, or opening balance.

### Focused validation

The generated VCT regression uses 48 deterministic seed transformations and
legal perturbations, filters OpenThree-or-stronger tactical states, and compares
production with the all-legal defender oracle at caps 3/5. It found no counterexample.
The oracle retains every AND defense; it can stop an ascending OR enumeration
once a three-ply proof reaches the independently established minimum distance.
This saves test work without weakening proof status, distance or canonical-PV
agreement. Production defender compression remains compact and unchanged.

Additional focused coverage exercises exact global limits, last-completed-depth
semantics, cancellation/deadlines, full state restoration, proof interruption
without cached disproof, local-budget fallthrough, observer snapshots, paired
Arena accounting, stale worker events and shutdown. Local-game tests cover
history/undo/terminal restoration, notation, record replay/rejection, every built-in
opening, human-decision undo floors and Undo/import request invalidation. The
known-loss regression covers two and multiple winning threats with exact score,
legal terminal PV and deterministic meaningful resistance.


Final validation completed successfully from the workspace root:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed, no warnings |
| `cargo test --workspace --all-features` | Passed: 148 tests (18 Core, 124 Engine, 2 Arena, 4 Native) |
| `cargo test --release -p rustmoku-engine` | Passed: 124 tests |
| `cargo build --release -p rustmoku-native` | Passed |
| `cargo build --release -p rustmoku-arena` | Passed |

The native validation is behavioral/session and worker testing, not a manual
visual acceptance run. Cooperative deadlines can overshoot with an expensive
evaluator/observer or OS scheduling. The opening suite has no measured balance
provenance, and this Arena smoke sample supplies no strength estimate.

## V0.9 Multi-core Lazy SMP / shared TT scaling (2026-09-06)

The V0.9 measurements use the official V0.8 baseline revision
`46bd2e7767d71bd9914af39e2d36520bb8ab6c6c`, the same Ryzen 7 8845H host and
Release toolchain documented above. Each run used PatternEvaluator, default
proof settings, depth 6, one untimed warm-up and three cold repetitions. Every
timed repetition clears the ordinary TT before starting; allocation and clearing
are outside timing. Results below are medians. The cap is disabled, so ordinary
search uses worker-local counters and aggregates them after all scoped helpers
join. `work` includes the 330 VCT root-inspection visits on these non-proven
fixtures; `p/h` is principal/helper Alpha-Beta nodes.

Commands for the matrix were equivalent to:

```powershell
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture opening --threads 8 --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture forced_defense --threads 8 --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture non_vct_tactical --threads 8 --repeats 3
```

The full sweep substituted `--threads 1,2,4,8,16` one value at a time.
`non_vct_tactical` is the additional nontrivial ordinary-search fixture; its
root VCT attempt is not proven and normal Alpha-Beta still runs.

| Fixture | T1: ms / work (p/h) | T2: ms / work (p/h) | T4: ms / work (p/h) | T8: ms / work (p/h) | T16: ms / work (p/h) |
|---|---:|---:|---:|---:|---:|
| opening, 129 / 19,180 | 111.939 / 123,704 (123,374/0) | 98.743 / 187,003 (96,105/90,568) | 65.805 / 243,266 (60,091/182,845) | 62.701 / 436,433 (58,910/377,193) | 81.277 / 829,754 (57,869/771,555) |
| forced_defense, 112 / -243,380 | 173.765 / 189,211 (189,211/0) | 127.456 / 293,802 (147,233/146,569) | 100.579 / 400,620 (95,245/305,375) | 76.686 / 588,918 (77,901/511,017) | 85.524 / 930,615 (62,685/867,930) |
| non_vct_tactical, 95 / 40,500 | 534.521 / 525,142 (524,812/0) | 374.932 / 794,794 (394,336/400,128) | 330.235 / 1,368,621 (350,596/1,017,695) | 307.903 / 2,605,062 (332,841/2,271,891) | 304.377 / 4,601,122 (304,577/4,296,215) |

All five worker counts retained the same completed depth, best move and score
for each fixture in this batch. Speedup and parallel efficiency (`T1/Tn` and
speedup divided by `n`) were:

| Fixture | T2 | T4 | T8 | T16 |
|---|---:|---:|---:|---:|
| opening | 1.13 / 56.7% | 1.70 / 42.5% | 1.79 / 22.3% | 1.38 / 8.6% |
| forced_defense | 1.36 / 68.2% | 1.73 / 43.2% | 2.27 / 28.3% | 2.03 / 12.7% |
| non_vct_tactical | 1.42 / 71.3% | 1.62 / 40.5% | 1.74 / 21.7% | 1.76 / 11.0% |

Thread 1 remains the V0.8 semantic and performance reference: opening is
129 / 19,180 at 111.939 ms versus the paired V0.8 111.623 ms, and forced defense
is 112 / -243,380 at 173.765 ms versus 176.776 ms. The small timing differences
are within normal host variation and do not show a greater-than-5% regression.
The helper work is intentionally extra exploration, so total work rises with
worker count. Eight workers were the fastest opening/forced-defense point here;
16 was slower on opening and had low efficiency. Lazy SMP is not expected to
scale linearly.

### Shared TT capacity at eight workers

Opening depth 6 was repeated with eight workers at 64, 256 and 512 MiB primary
capacity. The semantic result was 129 / 19,180 in all three runs. `tt_memory_mib`
describes primary bucket storage; the synchronization sidecar is included in
the reported allocation.

| Requested | Primary / sidecar / total | Buckets / entries | Nodes | Median ms | Hashfull / replacements |
|---:|---:|---:|---:|---:|---:|
| 64 MiB | 64 / 8 / 72 MiB | 1,048,576 / 4,194,304 | 399,443 | 59.850 | 5 / 0 |
| 256 MiB | 256 / 32 / 288 MiB | 4,194,304 / 16,777,216 | 446,697 | 64.542 | 1 / 0 |
| 512 MiB | 512 / 64 / 576 MiB | 8,388,608 / 33,554,432 | 419,679 | 59.976 | 0 / 0 |

The larger tables reduce sampled occupancy and collision pressure, but this
small depth-six workload shows no stable wall-time or node advantage; timings
are affected by scheduling and cache state. The default remains a 64 MiB
primary table (72 MiB including synchronization). A VCT fixture was also run at
threads 1 and 16: both returned the same 915-work, 5-ply VCT proof, confirming
that tactical root work remains coordinator-side rather than advertising VCT
parallel speedup.

The focused tests cover atomic payload/move round trips, same-bucket collisions,
concurrent writers, exact global admission across workers, legal principal PVs,
last-completed-depth cancellation behavior, helper shutdown as internal rather
than public cancellation, evaluator/state restoration and Native/Arena request
ownership. A final Release rerun after the lifecycle-state tightening retained
the same depth, move and score for every matrix row. Its medians were 53.353 /
39.813 / 28.826 / 27.724 / 36.044 ms for opening, 83.005 / 54.104 / 42.179 /
34.931 / 45.084 ms for forced defense, and 246.837 / 180.736 / 139.625 /
142.261 / 171.780 ms for non-VCT tactical at 1/2/4/8/16 workers. The matching
64/256/512 MiB opening-capacity medians were 24.924 / 25.049 / 24.089 ms;
primary/synchronization/total storage remained 64/8/72, 256/32/288 and
512/64/576 MiB. The scaling, capacity and VCT checks are intentionally
Release-only measurements, not ordinary tests.

Final V0.9 validation from the workspace root:

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets` | Passed |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --workspace --all-features` | Passed: 160 tests across Core, Engine, Arena and Native |
| `cargo test --release -p rustmoku-engine` | Passed: 134 tests |
| `cargo build --release -p rustmoku-native` | Passed |
| `cargo build --release -p rustmoku-arena` | Passed |
| Release quick/scaling/capacity/VCT benchmarks and threaded Arena smoke | Passed |

### Independent V0.9 review and repair (2026-09-06)

Reviewed the complete official V0.8 `46bd2e7767d71bd9914af39e2d36520bb8ab6c6c`
to V0.9 `d5c93a3c9686be805bd509e4b0935ecba027a24e` diff from a clean worktree,
then repaired two confirmed findings:

- **P0, ordinary TT publication:** `snapshot_bucket` could accept an old full
  key with a new writer's payload. Acquire version reads around Relaxed fields
  lacked a synchronization edge from the observed field to the writer's odd
  claim. A Loom run of the actual table reproduced `key=1, score=50000,
  depth=4, Exact` when key 1's published score was 1. Release field stores and
  Acquire field loads repair that edge; the argument is in ARCHITECTURE.md.
  Exhausted versions now reject stores until exclusive clear/resize, avoiding
  ABA without a reader-duration assumption. No unsafe code was added.
- **P2, benchmark sample association:** the driver paired the last search's
  result/counters/occupancy with the median of independently sorted times.
  Scheduling-dependent SMP work made the reported NPS and row inconsistent.
  It now selects one complete median-time sample. The earlier V0.9 timing
  medians remain timing measurements, but their adjacent work counts must not
  be interpreted as belonging to those median runs.

No further P1/P2 was confirmed in principal authority, exact global admission,
stop-reason arbitration, coordinator-only proof solving, state restoration,
directional BoundValidity, exact-depth/mate TT policy, or Native reconfiguration.
Internal team completion cannot overwrite a public stop or create Cancelled.
Scoped joins and exclusive engine reconfiguration exclude clear/resize races;
one generation spans the team and ordinary TT history survives game edits.
Search algorithms and their thread-one ordering were unchanged.

Validation used rustc 1.98.1, `x86_64-pc-windows-msvc`, Release, PatternEvaluator,
default proofs, 64 MiB primary TT, one untimed warm-up and three cold repetitions.
Official V0.8 was built from its archived commit in a separate directory;
unmodified V0.9's executable was saved before repair. Successive same-host runs
compared all 28 shared result/work fields over ten fixture/depth cases: quick's
five fixtures, three depth-six fixtures, VCF and VCT. They all matched, including
completed depth, seldepth, best move, score and every common search counter.
This is bounded regression evidence, not a universal minimax-equivalence claim.

| Thread-one workload | V0.8 ms | Reviewed V0.9 ms | Repaired ms | Best / score | Work |
|---|---:|---:|---:|---|---:|
| Quick, sum of five medians | 6.151 | 6.697 | 6.469 | All unchanged | 17,199 |
| opening D6 | 51.777 | 53.783 | 52.543 | 129 / 19,180 | 123,704 |
| forced_defense D6 | 80.419 | 82.400 | 82.881 | 112 / -243,380 | 189,211 |
| non_vct_tactical D6 | 237.851 | 244.116 | 240.388 | 95 / 40,500 | 525,142 |
| vct_win | 0.257 | 0.303 | 0.303 | 112 / 99,999,995 | 915 |
| vcf_win | 0.015 | 0.043 | 0.047 | 111 / 99,999,995 | 19 |

The ordinary depth-six repair deltas versus reviewed V0.9 are -2.3%, +0.6% and
-1.5%; there is no measured repair regression above 5%. Quick is 5.2% slower
than V0.8 but 3.4% faster than reviewed V0.9, so that small aggregate overhead
predates this repair. Very short proof measurements are dominated by setup and
timing noise and are not evidence of changed proof work.

The existing lean scaling sweep was rerun at D6 with the commands above,
substituting each thread count. Each cell below is median ms / work from the
**same** sample. Every row retained the same depth, move and score as thread one.

| Fixture | T1 | T2 | T4 | T8 | T16 |
|---|---:|---:|---:|---:|---:|
| opening | 53.474 / 123,704 | 38.637 / 187,439 | 26.722 / 250,070 | 25.527 / 386,399 | 37.442 / 795,815 |
| forced_defense | 82.207 / 189,211 | 62.889 / 303,529 | 43.585 / 396,910 | 36.627 / 672,284 | 45.729 / 1,188,912 |
| non_vct_tactical | 241.948 / 525,142 | 173.752 / 845,430 | 167.196 / 1,472,720 | 174.345 / 2,245,988 | 278.959 / 4,002,060 |

Eight workers gave 2.09x/2.24x on opening/forced defense. The non-VCT tactical
fixture was fastest at four; sixteen workers were slower than one in this batch.
Host load, scheduling and redundant helper work limit these measurements; no
linear scaling or strength claim follows. The 64-byte bucket has no guaranteed
cache-line alignment, adjacent version words can false-share, and colliding
evictions still increment one global atomic statistic. Those are scaling risks
under different occupancy/contention, not demonstrated bottlenecks in this lean
batch. Exact capped admission also necessarily synchronizes visits; uncapped
ordinary work retains local counters. No pool or speculative layout change was
introduced.

The T8 opening capacity check at 64/256/512 MiB retained 129 / 19,180; median
times were 40.239/41.462/33.707 ms, with primary/sidecar/total allocation still
64/8/72, 256/32/288 and 512/64/576 MiB. This separate late batch does not justify
cross-batch speedup comparisons. VCT at T1/T16 returned the same complete
five-ply proof, 915 work, and zero helper nodes. Arena smoke with two paired
openings, depth 2, global 2,000 nodes/move, A=T1 and B=T4 completed all four games
legally (184 searched moves, 307,311 work). Its B=4/A=0 result is not strength
evidence.

Executed successfully after repair:

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features` (161 tests)
- `cargo test --release -p rustmoku-engine` (135 tests)
- `cargo build --release -p rustmoku-native`
- `cargo build --release -p rustmoku-arena`
- `cargo test -p rustmoku-engine --example search_bench` (median association)
- `cargo test -p rustmoku-engine transposition_table::tests` (15 tests)
- Two actual-TT Loom tests: unbounded single-writer model and competing
  same-key/colliding writers with a preemption bound of two.
- Release quick, D6 scaling, capacity, VCF/VCT and threaded Arena smoke above.
- `git diff --check` and final diff review.

Reproduce the separate memory-model test build in PowerShell:

```powershell
$savedRustFlags = $env:RUSTFLAGS
try {
    $env:RUSTFLAGS = '--cfg loom'
    cargo test --release -p rustmoku-engine --lib tt_loom
} finally {
    $env:RUSTFLAGS = $savedRustFlags
}
```

Loom is only enabled in that explicit instrumented configuration; the normal
engine dependency tree still contains only Core. The model build emitted MSVC
import-library informational linker warnings; normal strict Clippy passed.
No weak-memory hardware run, saturated-table scaling profile, or manual Native
visual acceptance was performed. Loom coverage complements the publication
argument and does not exhaust every weak-memory execution or team size.
