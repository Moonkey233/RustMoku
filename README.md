# RustMoku

RustMoku is a small, deterministic Gomoku program and the V0.1 foundation of a
future engine research platform. The current release deliberately favors correct
game semantics, clear crate boundaries, and reproducible search over advanced
playing strength.

## V0.1 features

- 15 x 15 Freestyle Gomoku: five or more contiguous stones wins.
- Native Windows UI built with `eframe`/`egui`.
- Human versus AI play, with a Black or White side selection and New Game.
- Deterministic fixed-depth Negamax search with fail-soft Alpha-Beta pruning.
- A simple classical contiguous-pattern evaluator.
- Local candidate generation, tactical move ordering, and basic search statistics.
- Tests for the core game invariants and key engine behavior.

V0.1 does not implement Renju or Standard Gomoku, forbidden moves, opening
protocols, time controls, search cancellation, multithreading, tactical solvers,
transposition tables, learned evaluation, records, or network services.

## Prerequisites

- Windows with the MSVC Rust build prerequisites installed.
- `rustup`; the checked-in toolchain file selects the installed stable toolchain
  with `rustfmt` and `clippy`.

## Build and run

From the repository root:

```powershell
cargo build --workspace
cargo run --release -p rustmoku-native
```

The AI defaults to depth 4. Search is intentionally synchronous in V0.1, so the
window can pause briefly while the AI is choosing a move.

## Test and validate

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --release -p rustmoku-native
```

## Workspace

- `crates/rustmoku-core`: board, validated moves, rules, legal transitions, win
  detection, and game flow.
- `crates/rustmoku-engine`: evaluation, candidate generation, move ordering, and
  Alpha-Beta search.
- `apps/rustmoku-native`: desktop presentation and user interaction.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for invariants and search
semantics.

## Roadmap

Future milestones may improve classical pattern coverage, add measured search
optimizations and tactical solving, and later explore learned evaluation, match
automation, and deployment targets. Those capabilities are not part of V0.1 and
will be added only with concrete tests and evidence.
