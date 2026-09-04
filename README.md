# RustMoku

RustMoku V0.2 is a deterministic 15 x 15 Freestyle Gomoku program and a small
research-oriented engine foundation. It prioritizes correct game semantics,
clear crate boundaries, reproducible results, and measurable search behavior.

## V0.2 features

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
- A deliberately simple contiguous-pattern classical evaluator.
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

Local V0.1 and V0.2 measurements are recorded in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

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

V0.2 deliberately has no time control, cancellation, threads, aspiration
windows, PVS, killer/history heuristics, LMR, quiescence, tactical solvers,
incremental evaluation, NNUE, MCTS, opening book, server API, Renju, or Swap
protocol. The classical evaluator does not model broken threes/fours or compound
threats.

A suitable next milestone is measured search refinement—such as PVS and
aspiration windows—only after adding fixed-position regressions and comparing
them against the V0.2 baseline. Those features are not currently implemented.
