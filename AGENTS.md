# RustMoku Development Guidelines

RustMoku is a high-performance Gomoku engine and research platform written in Rust.

The long-term goal is to support strong classical search, tactical solvers, learned evaluation, automated engine matches, native applications, and server deployment. Development is incremental: preserve clean boundaries for future work, but do not implement speculative abstractions before they are required.

For general Rust engineering rules, also read:

* `docs/RUST_GUIDELINES.md`
* `docs/ARCHITECTURE.md`
* `docs/ROADMAP.md` (milestone scope and long-term direction)

Project-specific rules in this file take priority when they are more specific.

## 1. Project Priorities

Prefer, in order:

1. Correct game semantics.
2. Search correctness.
3. Clear module boundaries.
4. Safe and idiomatic Rust.
5. Deterministic and reproducible behavior.
6. Testability.
7. Algorithmic performance.
8. Data-layout and micro-optimizations only after measurement.

Do not sacrifice correctness for higher reported search depth or NPS.

## 2. Architecture Boundaries

The workspace is divided into independent layers.

`rustmoku-core` owns Gomoku domain logic:

* board state;
* moves;
* stones;
* rules;
* legal state transitions;
* win detection;
* game state.

`rustmoku-engine` owns AI logic:

* move generation;
* move ordering;
* evaluation;
* search;
* deterministic Zobrist hashing and the transposition table;
* future tactical solvers.

`rustmoku-native` owns desktop presentation and interaction.

Dependencies must remain one-way:

* `rustmoku-engine` may depend on `rustmoku-core`;
* `rustmoku-native` may depend on `rustmoku-core` and `rustmoku-engine`;
* `rustmoku-core` must not depend on engine, GUI, networking, or application code;
* `rustmoku-engine` must not depend on GUI or application code.

Do not move domain rules into the UI or search implementation for convenience.
Played-game history and opaque undo tokens belong to Game, never Position.
Human notation and record/opening replay semantics belong to Core. Native owns
I/O and its human-decision undo floor; imports/openings use legal Game replay.

## 3. Position Is the Engine Boundary

Search engines operate on `Position`, not GUI state or `Game`.

`Position` must contain everything required to evaluate a normal Gomoku position, including side to move and rule set.

Do not expose unrestricted mutable access to Position internals.

State changes must go through invariant-preserving operations such as `make_move` and `unmake_move`.

A search call receives an immutable caller-owned Position. The engine itself is mutable because it owns persistent search state. It may create one working Position copy at the search root, but recursive search must use make/unmake rather than cloning Position at every node. Engine-private hashes and caches belong in a search-side state, not in `rustmoku-core::Position`. SearchState owns exactly one always-available PatternState for ordering, qsearch, and evaluation; evaluator State/Undo hold evaluator-specific data only.

## 4. Domain Types

Use explicit domain types instead of interchangeable primitives when this prevents errors.

In particular:

* `Stone` represents only actual stones, not empty cells;
* empty cells are represented separately, normally with `Option<Stone>`;
* `Move` is a validated domain type and must not expose construction of out-of-board values;
* do not use sentinel moves such as `-1` or `255`;
* legitimate absence of a move is `Option<Move>`.

Keep representation details private when exposing them would allow invalid states.

## 5. Rule Design

Do not over-generalize rule handling.

For the initial version, model the supported rule set explicitly with an enum.

Opening protocols such as Swap or Swap2 are conceptually distinct from board win/forbidden-move rules and should not be mixed into ordinary win detection.

## 6. Search Semantics

The engine uses deterministic fail-soft PVS, aspiration iterative deepening, and bounded Gomoku threat quiescence. Root canonical ties require exact-score verification when a scout returns only a bound. Immediate tactical facts take priority over qsearch caps; potential Four-class threats are not mandatory defense. Reduced searches must retry nominal depth before improving alpha/PV, and unverified selective subtrees must not be stored as nominal-depth TT bounds.

Offline proof solving uses every legal move as its completeness universe. An
attacker OR proof needs one real proven child; a defender AND proof needs every
legal reply proven. Bounded failure, cancellation, tactical NoProof/NotProven,
and omitted progressively widened moves are Unknown, never Refuted. Persistent
Proof Books use collision-free D4 canonical identity outside the ordinary search
hot path. Parsed books are untrusted and cannot affect runtime search until full
strategy verification succeeds.

Evaluation scores are always documented with an explicit perspective. The initial evaluator returns scores from the side-to-move perspective.

Terminal win/loss values use mate-distance semantics so the engine prefers faster wins and delays forced losses.

Transposition-table probes may use a score or bound only when the stored depth is sufficient and the full Zobrist key matches. The fixed-depth baseline additionally requires equal remaining depth for score/bound reuse, because deeper heuristic values have a different horizon; legal TT moves may still order at any depth. Mate scores must be normalized by ply when stored and denormalized when probed. Cached moves must be validated before use. Quiescence scores must not share ordinary TT depth/bound semantics; history and killers remain per-public-search and subordinate to tactical ordering.

Do not use `i32::MAX` or `i32::MIN` directly as search infinity values when arithmetic or negation may be applied.

Do not add selective pruning techniques merely because they exist in chess or another engine. Any future pruning technique must have:

1. a documented rationale for Gomoku;
2. correctness/regression tests;
3. fixed-position benchmarks;
4. engine-vs-engine evidence when it can affect playing strength.

A public lifecycle interruption must preserve the last fully completed iteration's
move, score, PV and seldepth. Recursive stops are explicit results; restore every
make/unmake sidecar before propagating them and never cache interrupted work as
an ordinary bound or tactical disproof. Global work limits include normal/qsearch
and tactical proof/certificate visits; local proof exhaustion is distinct.

## 7. Determinism

The baseline engine is deterministic.

Given the same:

* Position;
* engine configuration;
* search limits;
* software version;

the engine should produce the same semantic result: best move and score. Ordinary searched root equal-score selection must use the canonical lower move
index. An exact known-loss tactical shortcut first prefers resistance at real
opponent winning points, then applies canonical index order within those points.

Persistent transposition state may change nodes, hit counts, and wall time between cold and warm searches. Use a fresh engine or explicitly clear the table for reproducible performance measurements. Zobrist generation and TT replacement must remain deterministic.

Move ordering must have a deterministic tie-break rule.

Do not introduce implicit randomness.

Future randomized play must use explicit configuration and an explicit reproducible seed.

## 8. Search Hot-Path Rules

Recursive search is performance-sensitive.

Avoid inside ordinary search nodes:

* cloning Position;
* `HashMap` or `HashSet`;
* formatted strings;
* logging;
* unnecessary heap allocation;
* dynamic dispatch where a stable static implementation is already known;
* synchronization primitives.

Use reversible make/unmake state transitions. Cross-thread cancellation may use
one Arc<AtomicBool> token per request with Relaxed polling at a fixed coarse
stride; do not clone the Arc or read the clock at every node.

Hot search state should be incrementally maintained when the affected region is naturally bounded and differential correctness tests exist.

Prefer bounded fixed-capacity data structures where the Gomoku board gives a natural hard bound.

Do not introduce unsafe code for speculative speedups.

## 9. Evaluation

Evaluation must remain separate from search.

The search implementation must not contain scattered hard-coded pattern values.

Expose evaluation through a narrow evaluator abstraction so later implementations such as classical pattern evaluation and NNUE can replace each other without rewriting search.

The initial evaluator may deliberately be slower and simpler if that makes it a reliable correctness reference.

## 10. GUI

The native GUI is an adapter.

It may:

* display Position and Game state;
* translate pointer interaction into Moves;
* invoke an engine;
* display SearchResult.

It must not:

* implement Gomoku win rules;
* inspect private search internals;
* duplicate evaluator logic;
* mutate Position internals directly.

Native search runs on one persistent standard-library worker that owns the engine.
Do not wrap the engine in Arc<Mutex<_>>. The UI must cancel and advance its request
ID on invalidation, reject every stale event, and cancel/shutdown/join on drop.
Application scheduling stays outside Core and Engine search semantics.

## 11. Unsafe Rust

The initial project forbids unsafe Rust.

Use:

```rust
#![forbid(unsafe_code)]
```

in first-party crates where practical.

Do not weaken this rule for hypothetical performance improvements.

Any future unsafe optimization requires profiling evidence, a documented safety argument, and isolated review.

## 12. Dependencies

Keep dependencies deliberate.

Do not add crates for trivial functionality already handled clearly by the standard library.

A dependency should have a current concrete use.

Do not add future NN, server, serialization, async, benchmarking, or parallelism dependencies before the corresponding feature exists.

## 13. Testing

Core invariants require tests.

At minimum, test:

* Move coordinate/index conversion;
* invalid Move rejection;
* legal and illegal state transitions;
* side-to-move changes;
* make/unmake round trips;
* horizontal, vertical, and both diagonal wins;
* Freestyle overlines;
* search input immutability;
* generated move legality and uniqueness;
* immediate tactical wins and blocks;
* deterministic search results.

Regression fixes should add regression tests.

When an optimized implementation later replaces a simple reference implementation, use differential tests where practical.

## 14. Performance Work

Do not judge engine performance using debug builds.

For performance changes:

1. establish a reproducible position or benchmark suite;
2. build in Release mode;
3. record nodes, depth, time, and relevant search statistics;
4. profile if necessary;
5. make one justified change;
6. rerun correctness tests;
7. compare against the baseline.

Intermediate benchmark regression is evidence to investigate, not by itself a reason to delete correctness-validated infrastructure with a concrete role in later search stages.

NPS alone is not an engine-strength metric.

Prefer fewer required nodes and better move ordering over cosmetic NPS improvements.

## 15. Code Quality

Follow the detailed Rust rules in `docs/RUST_GUIDELINES.md`.

In particular:

* use Safe Rust;
* keep ownership explicit;
* do not clone to bypass borrowing problems;
* do not overuse generics;
* do not annotate every obvious local type;
* do not hide warnings;
* keep public APIs intentionally small;
* use `rustfmt` as formatting authority.

Source identifiers, rustdoc, and technical comments should normally use clear English.

Comments should document invariants, intent, and non-obvious reasoning rather than restating syntax.

## 16. Required Validation

Before declaring an implementation complete, run from the workspace root:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Also run any feature-specific or project-specific tests relevant to the change.

Search-infrastructure changes must also run the Release engine tests and the fixed-position benchmark documented in `docs/PERFORMANCE.md`.

Do not claim a command passed unless it was actually executed successfully.

## 17. Scope Discipline

Implement the requested milestone, not the entire roadmap.

Preserve clean extension points where they are already justified, but avoid speculative infrastructure.

In particular, do not prematurely add:

* MCTS;
* NNUE;
* neural-network runtimes;
* server APIs;
* async runtimes;
* multithreaded search;
* opening books;
* Renju rules;
* Swap/Swap2;
* persistence;
* generic plugin systems;

unless the active milestone explicitly requires them.

A small correct implementation with stable boundaries is preferred over a large framework of unused abstractions.
