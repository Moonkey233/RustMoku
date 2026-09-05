# RustMoku

RustMoku V0.6 is a deterministic 15 x 15 Freestyle Gomoku program and a small
research-oriented engine foundation. It prioritizes correct game semantics,
clear crate boundaries, reproducible results, and measurable search behavior.

## V0.6 - Exact VCF Solver & Tactical Search Foundation

- Exact Freestyle continuous-four proofs: shortest proof, canonical equal-length
  choice, complete winning PV, and independent deterministic proof cache/budget.
- Gated root VCF attempt before classical search; GUI displays proof distance
  without inventing completed Alpha-Beta depth.
- Explicit LMR result completeness governs TT storage; forced-block nodes can
  use valid equal-depth TT scores while keeping exactly one candidate.
- Freestyle Gomoku: five or more contiguous stones wins, with no forbidden moves
  or opening protocol.
- Native Windows human-versus-AI UI built with `eframe`/`egui`, including Black
  or White selection and New Game.
- Fail-soft PVS with iterative deepening and exponentially widened aspiration
  windows; canonical equal-score root selection is preserved.
- Per-search history and two killers per ply, subordinate to tactical priorities.
- Cached profile bitsets and exact immediate tactics: win in one, unique forced
  block, or loss in two against multiple winning points, shared by all searches.
- Threat quiescence expands only own Four-or-stronger continuations through
  bitsets. Its six-ply expansion cap never masks immediate wins or forced replies;
  potential opponent Four+ threats do not remove stand pat.
- Conservative one-ply LMR for late, low-history Quiet moves at scout nodes;
  improvements are re-searched at nominal depth before updating alpha/PV.
- Deterministic incremental 64-bit Zobrist keys and a persistent, fixed-size,
  four-way clustered transposition table.
- Mate-distance-safe TT scores, TT-aware tactical move ordering, and canonical
  equal-score root move selection.
- Legal principal-variation prefixes plus nodes/qnodes, LMR/re-search, evaluation,
  cutoff, and TT statistics.
- Engine-private bitboards and an incremental radius-two candidate frontier.
- Incremental semantic line/threat patterns, including broken and compound shapes,
  cached tactical ordering, and constant-time aggregate evaluation.
- Private BoardState owns Position, hash, frontier, and one `PatternState`,
  shared by ordering, quiescence, VCF, and the
  default `PatternEvaluator` (unit evaluator state);
  `ClassicalEvaluator` remains the independent full-board reference implementation.
- Wrapping TT generations, depth/Exact/age-weighted replacement, bounded hashfull sampling,
  configurable capacity/resize, and collision-replacement statistics.
- Tests covering domain invariants, hashing, TT behavior, search semantics, and
  determinism.

## Prerequisites

- Windows with the MSVC Rust build prerequisites installed.
- `rustup`; [`rust-toolchain.toml`](rust-toolchain.toml) selects stable Rust with
  `rustfmt` and `clippy`.

## Build, test, and run

From the repository root:

```powershell
cargo build --workspace
cargo test --workspace --all-features
cargo run --release -p rustmoku-native
```

The AI defaults to depth 4, a 64 MiB transposition table, and a gated VCF attempt
limited to 11 proof plies / 2,000 nodes with a separate 384 KiB proof table. Search remains
synchronous, so the window can pause briefly while the AI chooses a move.

The permanent fixed-position benchmark utility is run with:

```powershell
cargo run --release -p rustmoku-engine --example search_bench
```

Use `--suite deep`, `--depth 8`, `--fixture opening`, `--tt-mib 256`,
`--repeats 3`, or `--evaluator classical` after Cargo's `--` separator. Defaults
are the historical depth-four suite, 64 MiB, PatternEvaluator, one warm-up and
five cold runs, reporting their median. TT allocation/clearing is untimed.
For example:

```powershell
cargo run --release -p rustmoku-engine --example search_bench -- --suite quick --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture opening --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture forced_defense --repeats 3
```

To use the reference engine explicitly:

```rust
use rustmoku_engine::{AlphaBetaEngine, ClassicalEvaluator};
let mut engine = AlphaBetaEngine::new(ClassicalEvaluator);
```

The V0.6 lean performance check against official V0.5 `faf36748` and historical
measurements are recorded in [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).
Use `--fixture vcf_win` or `--fixture non_vcf_tactical` for the two additional
V0.6 fixtures; the default quick suite still contains the original five positions.
`--vcf-plies` and `--vcf-nodes` override deterministic proof limits; zero disables VCF.

## Full validation

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --release -p rustmoku-engine
cargo build --release -p rustmoku-native
cargo run --release -p rustmoku-engine --example search_bench
```

## Workspace

- `crates/rustmoku-core`: board, validated moves, rules, legal transitions,
  cached win state, and game flow.
- `crates/rustmoku-engine`: evaluation, candidates, ordering, Zobrist hashing,
  transposition table, principal variation, and PVS/threat search.
- `apps/rustmoku-native`: desktop presentation and interaction adapter.

Dependencies remain one-way: Engine depends on Core; Native depends on Core and
Engine. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the invariants and
search contracts.

## Current limits and roadmap

V0.6 has no VCT/TSS/DFPN, additional selective pruning, time control,
cancellation, threads, NNUE, MCTS, opening book, server API, Renju, or Swap protocol.
LMR remains heuristic and may miss quiet resources; it does not prove equality
with full-depth minimax. Quiescence omits ordinary Three expansion and optional
non-immediate defensive moves, and stops non-immediate forcing continuations at
the cap. Pattern weights and LMR thresholds are untuned. The Native app remains
synchronous; fixed-position timings do not establish playing strength.

VCF proves continuous-four wins only. NotProven is not a loss verdict; exhaustion
follows the existing classical search. Evaluator stays replaceable; its public
PatternState API debt and future milestones are recorded in
[`docs/ROADMAP.md`](docs/ROADMAP.md).
