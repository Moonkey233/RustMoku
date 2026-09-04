# RustMoku Search Performance

These measurements are local relative baselines. They are useful for comparing
search revisions on this machine, but they are not cross-machine benchmarks and
must not be interpreted as engine-strength ratings. Wall-clock results naturally
vary with machine load; semantic results and node counts are more stable.

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
- Repository base revision: `5a75ca3`, with the V0.2 working-tree changes under
  review
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
