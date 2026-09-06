# RustMoku

RustMoku V0.10 is a 15 x 15 Freestyle Gomoku program and a small
research-oriented engine foundation. It prioritizes correct game semantics,
clear crate boundaries, reproducible single-thread results, and measurable
search behavior.

## V0.10 - Threat-Aware Selectivity, History & QSearch

- Signed gravity history with malus, countermoves, and one/two-ply continuation
  history feeds the existing packed deterministic move order.
- Adaptive depth/index LMR protects tactical, TT, killer, countermove, and
  high-context moves; improving reduced searches retry at nominal depth.
- Guarded LMP, move futility, reverse futility, razoring, IIR, exact mate-distance
  bounds, and a one-ply path-bounded FourThree-or-stronger extension reduce work.
- Directional bound validity prevents every heuristic shortcut from publishing
  unsupported ordinary-TT Exact, Lower, or Upper evidence.
- Qsearch reports recursive work, forcing edges, forced blocks, stand-pat
  cutoffs, cap hits, and maximum qply. Measurement showed recursive expansion is
  small, so the exact Four+ vocabulary and mandatory-response semantics remain.

## V0.9 - Multi-Core Lazy SMP & Shared TT

- CPU-only Lazy SMP: one principal worker plus independent helper workers.
- `EngineConfig::threads()` defaults to 1; thread 1 preserves the deterministic
  reference mode, while larger teams are schedule-dependent by design.
- Worker 0 alone publishes `SearchInfo` and determines the public result; helpers
  explore the same root and populate a shared ordinary TT.
- The shared TT uses four atomic key/payload slots per bucket and an atomic
  bucket-version seqlock with Release/Acquire field publication and full-key
  validation, preventing mixed snapshots from being accepted as cutoffs.
- One global node admission counter is exact when `SearchLimits::max_nodes` is
  enabled; uncapped workers keep local counters that are aggregated after join.
- VCF and VCT remain single, coordinator-side root stages and are never repeated
  by Lazy-SMP helpers.
- Native exposes Threads and TT MiB controls; Arena accepts independent
  `--a-threads` and `--b-threads` settings.

## V0.8 - Search Lifecycle, Arena & Async Native

- Per-move depth, total work-node and elapsed-time limits; one-way cancellation.
- Explicit termination reason and last-completed-iteration score/PV on interruption.
- Coarse `SearchInfo` observers after completed depths and exact tactical proofs.
- A persistent native worker owns the engine; cancelled/stale requests cannot play.
- Single-threaded headless Arena with deterministic openings and paired colors.
- Game-owned ordered history, repeatable human-decision Undo and opening undo floors.
- Shared A1..O15 notation, versioned records, coordinates and optional move numbers.
- Stable board layout with a single scrolling PV row and bounded move history.
- Generated all-legal VCT defender audit found no counterexample; DFPN preserves
  useful partial entries against fresh unknown writes.

The established engine foundation remains:

- Exact bounded VCT with attacker OR / defender AND semantics, an explicit threat
  descriptor, and a separate build-generated 512 KiB tactical metadata table.
- Depth-first proof-number search with a dedicated context-sensitive tactical
  cache; parity depth limits and certificate reconstruction give fastest attacks,
  slowest defenses, canonical ties, and a complete terminal PV.
- Root order: exact immediate facts, cheaper VCF, gated VCT, then Alpha-Beta.
- Bound-aware LMR validity permits verified Lower cutoffs despite earlier selective
  fail-lows. VCF skips impossible parity depths and reuses validated shorter proofs.

- Exact Freestyle continuous-four proofs: shortest proof, canonical equal-length
  choice, complete winning PV, and independent deterministic proof cache/budget.
- GUI displays VCF/VCT proof distance
  without inventing completed Alpha-Beta depth.
- Result-owned directional validity governs TT storage; forced-block nodes can
  use valid equal-depth TT scores while keeping exactly one candidate.
- Freestyle Gomoku: five or more contiguous stones wins, with no forbidden moves
  or opening protocol.
- Native Windows human-versus-AI UI built with `eframe`/`egui`, including Black
  or White selection and New Game.
- Fail-soft PVS with iterative deepening and exponentially widened aspiration
  windows; canonical equal-score root selection is preserved.
- Per-search signed main/continuation history, countermoves, and two killers per
  ply, subordinate to tactical priorities.
- Cached profile bitsets and exact immediate tactics: win in one, unique forced
  block, or loss in two against multiple winning points, shared by all searches.
- Threat quiescence expands only own Four-or-stronger continuations through
  bitsets. Its six-ply expansion cap never masks immediate wins or forced replies;
  potential opponent Four+ threats do not remove stand pat.
- Adaptive LMR and conservative shallow selectivity for quiet scout-node work;
  exact tactics and strong threats remain protected.
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
  shared by ordering, quiescence, VCF, VCT, and the
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

The Native app displays its Cargo package version automatically and defaults to
the practical human-play profile: depth 8, Auto threads capped at eight logical
workers, a 128 MiB primary transposition table, 15,000 ms per move, move numbers
on, and Auto language. Auto selects Simplified Chinese for supported Chinese
system locales and English otherwise; users can explicitly select English or
Simplified Chinese. Native loads an existing Windows CJK font when needed and
safely falls back to English if none is available.

Library and Arena defaults remain the deterministic research profile: depth 4,
one Alpha-Beta worker, and a 64 MiB primary transposition table. The engine also
defaults to a gated VCF attempt limited to 11 proof plies / 2,000 nodes with a
separate 384 KiB proof table. VCT defaults to 9 plies / 4,000 node
inspections and a 16 MiB memory request (12 MiB actual bucket allocation). Roots
without OpenThree-or-stronger candidates spend zero VCT nodes. A persistent
worker owns the engine and ordinary TT. The UI remains responsive, displays
completed search snapshots, and accepts New Game while searching. Depth,
Auto/manual thread count, TT MiB and move time apply to the next request; setting
move time to 0 ms means unlimited. Each invalidation cancels its token and advances the request ID; both
old snapshots and old results are ignored. Application drop cancels, sends
shutdown and joins the worker.

The permanent fixed-position benchmark utility is run with:

```powershell
cargo run --release -p rustmoku-engine --example search_bench
```

Use `--suite deep`, `--depth 8`, `--fixture opening`, `--tt-mib 256`,
`--threads 4`, `--repeats 3`, or `--evaluator classical` after Cargo's `--`
separator. Defaults are the historical depth-four suite, 64 MiB, PatternEvaluator,
one warm-up and five cold runs, reporting their median. TT allocation/clearing is
untimed.
For example:

```powershell
cargo run --release -p rustmoku-engine --example search_bench -- --suite quick --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture opening --repeats 3
cargo run --release -p rustmoku-engine --example search_bench -- --depth 6 --fixture forced_defense --repeats 3
```

The V0.9 scaling sweep uses `--depth 6 --fixture opening|forced_defense|non_vct_tactical`
with `--threads 1`, `2`, `4`, `8` and `16`; 64/256/512 MiB capacity results and
the measured medians are recorded in [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md).

To use the reference engine explicitly:

```rust
use rustmoku_engine::{AlphaBetaEngine, ClassicalEvaluator};
let mut engine = AlphaBetaEngine::new(ClassicalEvaluator);
```

The V0.8 lean performance check and V0.9/V0.10 measurements are recorded in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md). Additional fixtures are `vcf_win`,
`vct_win`, and `non_vct_tactical`; the default quick suite still has five positions.
Use `--vcf-plies`, `--vcf-nodes`, `--vct-plies`, `--vct-nodes`, and `--vct-mib`.
Zero in a solver's ply/node limit disables that solver.

The concrete `TacticalConfig` groups `ProofLimits` for both solvers and VCT memory.
Convenience setters remain available:

```rust
use rustmoku_engine::EngineConfig;
let config = EngineConfig::new(64)
    .with_vcf_limits(11, 2_000)
    .with_vct_limits(9, 4_000)
    .with_vct_table_memory(16);
```

## Local games, Undo and records

Choose Empty Board or one of twelve built-in Freestyle test openings. "Next in
suite" cycles their fixed order with no hidden RNG. These are hand-authored
starts, with no official or measured balance claim. Core instantiates every start
by legal replay, and Arena uses the same suite for both legs of each pair.

"Undo turn" returns to the previous human decision: it removes one human move
while the AI is thinking, or the human move and completed AI reply. It handles
terminal games and either human color. An AI's initial move has no earlier human
decision to undo. Opening moves form the session's undo floor and remain in the
complete exported history. Core `Game::undo()` / `undo_plies(n)` are generic LIFO
operations; `Game::history()` exposes only chronological Moves. Position remains
history-free. Undo/import/New Game invalidate the worker request without clearing
its persistent ordinary TT.

Coordinates include I: A1 is bottom-left, O15 top-right and H8 center. Move's
`Display` / `FromStr` implementation is the authoritative codec; parsing also
accepts lowercase. PV, history, last-move text, board labels, Arena opening text
and records use it. The fixed controls/history panels keep the board stable
while live PV changes. PV scrolls horizontally in one row; history scrolls
vertically. "Move numbers" derives its overlay from Game history.

"Game record..." exports the current complete move sequence, copies text to the
clipboard, imports pasted text, and loads/saves an explicit file path using the
standard library. Loaded text is imported only when "Import text" is clicked;
invalid imports leave the current game untouched. Import starts an editable
session with undo floor zero and starts the AI if required by the chosen human
side. Files use deterministic canonical text:

```text
RustMoku 1
rules=freestyle
moves=H8 H9 G8 I8
```

`Game::from_record` creates a fresh Game and legally replays all moves. Unsupported
versions/rules, malformed coordinates, repeated occupied moves and moves after
termination produce contextual errors. `Game::to_record` includes all opening
and played moves with a final newline; it never serializes Position internals.

## Search lifecycle

```rust
use std::time::Duration;
use rustmoku_engine::{AlphaBetaEngine, CancellationToken, SearchEngine, SearchInfo, SearchLimits};
let mut engine = AlphaBetaEngine::default();
let position = rustmoku_core::Position::default();
let limits = SearchLimits::new(8)
    .with_max_nodes(100_000)
    .with_move_time(Duration::from_millis(500));
let token = CancellationToken::new(); // retain a clone on the controlling thread
let result = engine.search_controlled(&position, limits, token, &mut |info: SearchInfo| {
    println!("depth {} work {} score {}", info.completed_depth, info.statistics.work_nodes, info.score);
});
println!("{:?}", result.termination);
// Ordinary fixed-depth callers still use engine.search(&position, SearchLimits::new(4)).
```

`SearchTermination` is `Completed`, `NodeLimit`, `TimeLimit`, or `Cancelled`.
Interrupted iterations never replace the last completed score, move, PV or
seldepth. Final statistics include all spent work, including the discarded
iteration. Before any positive-depth iteration completes, the fallback is the
lowest candidate index (center on an empty board), a static score and one-move
PV, with completed depth zero. Zero-depth calls remain analysis-only: no move.
Known immediate exact tactics and terminal facts remain valid even at a stop;
VCF/VCT certificates are accepted only when complete. A cancelled GUI result is
never played.

`statistics.work_nodes` counts normal nodes including qnodes once, VCF visits
and certificate replay, and VCT/DFPN inspections and certificate visits. A global
node cap admits at most that many visits across all subsystems. Local proof
budget exhaustion still allows ordinary search; outer interruption stops it.
The cancellation atomic and optional clock are polled every 256 work nodes and
at root-stage/iteration boundaries. Time limits are cooperative per move, not
full-game clocks; node limits with fresh engines are the reproducible option.
Observers should return promptly because their time is part of the search.

`EngineConfig::threads()` is the total number of CPU Alpha-Beta workers in one
search. Worker 0 is authoritative for completed depth, result, PV and
`SearchInfo`; helpers only add useful shared-TT work. VCF and VCT run once on the
coordinator before the team. With one thread, the semantic baseline is
deterministic. With multiple threads, no RNG is used, but shared-TT timing and
OS scheduling may change heuristic search work and results.

The ordinary TT keeps four atomic key/payload slots per bucket and a separate
atomic u64 bucket-version sidecar. Writers claim an odd version, write the
selected key and packed payload with Release, then publish the next even version
with Release. Readers use Acquire for both fields and versions, accepting only
an unchanged even version and a full-key match. Versions never wrap under shared
access; exhausted buckets drop stores until exclusive clear/resize. A 64 MiB primary
table uses 8 MiB of sidecar versions, for 72 MiB of reported table storage.

## Headless Arena

```powershell
cargo run --release -p rustmoku-arena -- --pairs 2 --depth 2 --nodes 2000 --b-vct-nodes 0
cargo run --release -p rustmoku-arena -- --help
```

Each of up to twelve fixed opening prefixes is played twice: A as Black, then A
as White. Each game starts with fresh engines; each engine retains its TT between
moves. All moves go through Core
`Game`. No randomness, wall-clock adjudication, or GUI dependency is involved.
Players may use independent CPU thread counts. CSV goes to stdout; configuration
and A/B wins, draws, A's paired points and average work per move go to stderr.
A win scores one point and a draw half a point, so each pair has two points.

Player options use `--a-` or `--b-`: `evaluator pattern|classical`, `threads`, `tt-mib`,
`vcf-plies`, `vcf-nodes`, `vct-plies`, `vct-nodes`, and `vct-mib`. Common `--depth`
(default 3) and optional `--nodes` apply per move. Zero proof plies/nodes disables
a solver. Tiny paired runs validate the harness; they do not establish Elo or
engine superiority. Reproduce with the same revision, openings, configurations,
and limits.

## Full validation

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test --release -p rustmoku-engine
cargo build --release -p rustmoku-native
cargo build --release -p rustmoku-arena
cargo run --release -p rustmoku-engine --example search_bench
```

## Workspace

- `crates/rustmoku-core`: board, validated moves, rules, legal transitions,
  cached win state, and game flow.
- `crates/rustmoku-engine`: evaluation, candidates, ordering, Zobrist hashing,
  transposition table, principal variation, and PVS/threat search.
- `apps/rustmoku-native`: desktop presentation and persistent search worker.
- `apps/rustmoku-arena`: deterministic paired engine matches, no GUI dependency.

Dependencies remain one-way: Engine depends on Core; Native depends on Core and
Engine, as does Arena. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the invariants and
search contracts.

## Current limits and roadmap

V0.10 does not add Null Move, ProbCut, singular extension, interior VCF/VCT,
NNUE, MCTS, opening book, server API, Renju, or Swap protocol. Parallel results
with more than one worker are not promised bit-for-bit stable. Selective search
does not prove equality with full-width minimax. Quiescence omits ordinary Three expansion and optional
non-immediate defensive moves, and stops non-immediate forcing continuations at
the cap. Pattern weights and LMR thresholds are untuned. Cancellation latency
also depends on evaluator/observer cost; deadlines are not hard real-time guarantees.
Fixed-position timings do not establish playing strength.

VCF proves continuous-four wins; VCT admits Five, OpenFour, DoubleFour, FourThree,
DoubleThree, Four, and OpenThree attacks. Ordinary Three and arbitrary quiet
attacks are excluded. Reported distance is exact within that forcing vocabulary,
not unrestricted full-game minimax. NoProof/NotProven is not a loss verdict;
when two immediate opponent wins already establish an exact loss, the engine
blocks the first canonical threat point and reports the second as the terminal
reply. Score/distance stay exact; unrelated top-left moves are no longer chosen.
Local proof exhaustion remains Unknown and falls through to classical search. Proof numbers
saturate safely; practical node limits are far below that numerical ceiling.
Evaluator stays replaceable; its public
PatternState API debt and future milestones are recorded in
[`docs/ROADMAP.md`](docs/ROADMAP.md).
