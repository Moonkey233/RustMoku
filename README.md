# RustMoku

RustMoku V0.3 is a deterministic 15 x 15 Freestyle Gomoku program and a small
research-oriented engine foundation. It prioritizes correct game semantics,
clear crate boundaries, reproducible results, and measurable search behavior.

## V0.3 features

- Freestyle Gomoku: five or more contiguous stones wins, with no forbidden moves
  or opening protocol.
- Native Windows human-versus-AI UI built with `eframe`/`egui`, including Black
  or White selection and New Game.
- Stateful deterministic Negamax with fail-soft Alpha-Beta and full-window
  iterative deepening.
- Deterministic incremental 64-bit Zobrist keys and a persistent, fixed-size,
  four-way clustered transposition table.
- Mate-distance-safe TT scores, TT-aware tactical move ordering, and canonical
  equal-score root move selection.
- Principal variation plus node, evaluation, cutoff, and TT statistics.
- Engine-private bitboards and an incremental radius-two candidate frontier.
- Incremental semantic line/threat patterns, including broken and compound shapes,
  cached tactical ordering, and constant-time aggregate evaluation.
- V0.3 default evaluator = incremental `PatternEvaluator`;
  `ClassicalEvaluator` remains the independent full-board reference implementation.
- Wrapping TT generations, relative-age replacement, bounded hashfull sampling,
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

The AI defaults to depth 4 and a 64 MiB transposition table. Search remains
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
cargo run --release -p rustmoku-engine --example search_bench -- --suite deep
cargo run --release -p rustmoku-engine --example search_bench -- --depth 8 --fixture opening --tt-mib 1024 --repeats 3
cargo run --release -p rustmoku-engine --features bench-internals --example hotpath_bench -- 100000
```

To use the reference engine explicitly:

```rust
use rustmoku_engine::{AlphaBetaEngine, ClassicalEvaluator};
let mut engine = AlphaBetaEngine::new(ClassicalEvaluator);
```

Historical V0.1/V0.2 and measured V0.3 results are recorded in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md). The implementation and validation
report is [`docs/V0.3_REPORT.md`](docs/V0.3_REPORT.md).

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
  transposition table, principal variation, and Alpha-Beta search.
- `apps/rustmoku-native`: desktop presentation and interaction adapter.

Dependencies remain one-way: Engine depends on Core; Native depends on Core and
Engine. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the invariants and
search contracts.

## Current limits and roadmap

V0.3 deliberately has no time control, cancellation, threads, PVS, aspiration
windows, killer/history heuristics, LMR, quiescence, tactical solver, NNUE, MCTS,
opening book, server API, Renju, or Swap protocol. Pattern features are bounded
Freestyle continuation summaries; their initial weights are untuned. The Native
app remains synchronous. A suitable V0.4 would measure node reductions from PVS,
aspiration, ordering refinements, and then separately justified selective search.
Those changes require regression and playing-strength evidence and are not part
of V0.3. Intermediate benchmark regressions are evidence to investigate, not an
automatic reason to remove validated search infrastructure.
