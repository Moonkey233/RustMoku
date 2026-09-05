# RustMoku V0.3 Architecture

V0.3 adds incremental board intelligence to the V0.2 search infrastructure.
Core remains authoritative for legality and wins; Engine maintains reversible
private caches; Native remains an adapter. All first-party crates forbid unsafe
code. No new dependency, pruning algorithm, thread, or tactical solver is added.

## Crate dependency graph

```text
rustmoku-engine -> rustmoku-core
rustmoku-native -> rustmoku-core
rustmoku-native -> rustmoku-engine
```

`rustmoku-core` has no third-party dependencies and owns Gomoku semantics.
`rustmoku-engine` does not depend on presentation code. `rustmoku-native` draws
public state, maps clicks to validated moves, calls `Game`, and invokes the
engine. No reverse dependency exists.

## Core domain invariants

### Move

`Move` is a compact validated type with a private `u8`. Public constructors
accept only a board index in `0..225` or zero-based row and column in `0..15`.
`Move::all()` enumerates all 225 points and `Move::CENTER` is the center. There is
no sentinel move; absence is `Option<Move>`.

### Position

`Position` owns 225 `Option<Stone>` cells, side to move, `RuleSet`, move count,
last move, and a cached `Option<Stone>` winner. Its fields are private and it
provides no unrestricted mutable board access.

Every legal state maintains:

- move count equals board occupancy;
- Black starts and side to move switches after each move;
- `last_move` identifies the latest placed stone, or is `None` initially;
- `winner` is `None` initially and otherwise identifies the winner created by
  the latest legal move;
- occupied, full, and already-won positions reject another move;
- the active rule set travels with the position.

`make_move` updates the winner only by checking the new stone along the four
relevant axes. Consequently `winner()` is O(1); a full-board draw remains a
separate `is_full()` condition.

### Make/unmake

`Position::make_move` returns an opaque, non-cloneable `MoveUndo` containing the
state required to reverse that transition, including the previous last move and
winner. `unmake_move` consumes the token and restores the exact prior position.

An undo token must be used on its corresponding logical state in strict LIFO
order. Violation is programmer error. Debug builds detect common state/token
mismatches, but the token deliberately carries no global identity and cannot
guarantee detection of arbitrary cross-position misuse. These audit checks are
`debug_assert` operations and do not burden Release search nodes.

### Rules and Game

`RuleSet::Freestyle` is the only implemented ruleset. Five or more contiguous
stones wins; horizontal, vertical, backslash diagonal, and slash diagonal are
checked. There are no forbidden moves or Swap protocols.

`Game` owns real-game status (`Ongoing`, `Won`, or `Draw`) separately from
`Position`. Search consumes `&Position`, never `Game` or GUI state.

## Engine boundary and ownership

The intentionally small public surface includes `Evaluator`, `ClassicalEvaluator`,
`PatternEvaluator`, opaque `PatternState`/`PatternUndo`,
`SearchEngine`, `AlphaBetaEngine`, `EngineConfig`, `SearchLimits`,
`SearchResult`, `SearchStatistics`, and `TranspositionTableStatistics`. Candidate lists, ordering, hashes,
search-side state, TT entries, and PV tables remain private.

`SearchEngine::search` takes `&mut self` because `AlphaBetaEngine` owns a mutable,
persistent transposition table and generation counter. The caller still supplies
an immutable `&Position`, which search never mutates. No interior mutability,
synchronization primitive, background task, or global cache is involved.

## SearchState: coordinated incremental ownership

```text
AlphaBetaEngine<E>                  immutable evaluator configuration + mutable TT
  SearchState<E>                   one local working state per public search
    Position                      authoritative rule state (one root clone)
    PositionKey                   incremental Zobrist key
    CandidateFrontier             occupancy bits, frontier bits, neighbor counts
    E::State                      PatternState for the default evaluator
    optional ordering PatternState (only when E has no pattern cache)
```

Every recursive transition goes through `SearchState::make_move`/`unmake_move`.
Make asks Core to accept the move before changing any sidecar. A rejected Core
move therefore leaves all caches untouched. `SearchUndo<E::Undo>` owns the Core
undo, evaluator undo, optional ordering undo, played move, and stone. Undo is
consumed in LIFO order and restores every field. Recursion never clones Position.
The generic transition takes `&E` so engine configuration stays immutable while
the engine's TT can be borrowed mutably. No `dyn`, interior mutability, or locks.

### Stateful evaluation

`Evaluator` has associated `State` and `Undo`, and `initialize`, `make_move`,
`unmake_move`, `evaluate` lifecycle methods. `evaluate(&Position, &State)` returns
side-to-move-relative static scores. The transition callbacks are infallible for
accepted Core moves; callers must preserve the lifecycle and LIFO contract.

`ClassicalEvaluator` keeps `State = ()` and `Undo = ()`, retaining the independent
full-board contiguous-run reference implementation. `PatternEvaluator` owns a
`PatternState` and reverses transitions with a two-byte `PatternUndo` (Move and
Stone). Its optional `cached_patterns(&State)` hook shares that cache with move
ordering. The hook's presence must stay constant during the lifecycle. For
ClassicalEvaluator or another evaluator without patterns, SearchState maintains
one separate incremental ordering cache. The default has no duplicate cache.

### Zobrist

The root computes a full 64-bit key once; transitions toggle occupancy and side
with constant-time XOR. Tables are compile-time SplitMix64 values from the fixed
V0.2 seed. `(Stone, Move)`, side-to-move, and rules all participate. Keys/caches
remain outside Core. Full-recompute hash assertions are debug-only.

## BitBoard256 and CandidateFrontier

`BitBoard256([u64; 4])` maps bits directly to `Move.index()`. Bits 225..255 are
always zero; the fourth playable word contains 33 bits. Set/clear/test take
validated Moves. Union, intersection, and and-not preserve the invariant.
Iteration uses trailing zeros and `word &= word - 1`, yielding ascending indices.
A compile-time validated Move table avoids constructing Results during iteration.
Core Move constructors are now `const fn`, with unchanged checks and semantics.

`RADIUS2_MASKS[225]` is a deterministic compile-time table of Chebyshev-radius-two
cells, including the center. CandidateFrontier owns occupied/frontier bitboards
and `[u8; 225]` neighbor counts. Every count is at most 25. Make increments at most
25 counts and unions the mask; undo decrements them and clears bits at zero.
Overlapping neighbors survive undo correctly. Candidates are
`frontier & !occupied & PLAYABLE`, except the empty board yields CENTER. The
SearchState boundary returns no candidates for terminal/full boards.

There is no recursive geometric loop, occupied-cell scan, heap allocation, or
225-cell boolean reconstruction. The V0.2 geometric generator exists only for
tests and the opt-in `bench-internals` driver.

## Directional geometry and semantic pattern tables

Directions are horizontal `(0,1)`, vertical `(1,0)`, backslash `(1,1)`, and slash
`(1,-1)`. The candidate is the omitted center of a nine-cell window. `LineKey(u16)`
encodes offsets `[-4,-3,-2,-1,+1,+2,+3,+4]` in consecutive two-bit fields, least
significant first: Empty=00, Black=01, White=10, Wall=11. Walls are never empties.
`LINE_CELLS[225][4][8]` fixes all boundary geometry at compile time.

The independent slow classifier in `line_classifier.rs` places a color at the
center, then enumerates legal empty continuations inside the nine-cell window.
Every counted five includes the original center. Distinct next cells are counted
once, even when they complete multiple five-windows. Freestyle overlines win.
The highest applicable directional class is:

| Code | Class | Meaning after hypothetical center placement |
|---:|---|---|
| 5 | Five | A five already passes through the center |
| 4 | OpenFour | At least two distinct next moves make such a five |
| 3 | Four | Exactly one next move makes such a five |
| 2 | OpenThree | No immediate continuation wins, but some next move creates at least two winning continuations |
| 1 | Three | No stronger class; some next move creates at least one winning continuation |
| 0 | Quiet | None of the above |

Broken shapes follow the same simulation as contiguous ones; no regex or string
recognition runs in the engine. These are bounded directional features, not a
proof of a forced win against all defenses, and contain no Renju restrictions.

Cargo's dependency-free `build.rs` generates the 65,536 entries deterministically
into OUT_DIR. A `PatternPair` is two bytes (Black then White), making the embedded,
64-byte-aligned contiguous table exactly 131,072 bytes. There is no runtime table
initialization, synchronization, or classifier call. Exhaustive tests verify the
binary table against the semantic classifier, color symmetry, and reflection.

### ThreatProfile

A one-byte Copy enum aggregates four directions, in ascending ordering strength:
Quiet, Three, OpenThree, DoubleThree, Four, FourThree, DoubleFour, OpenFour,
WinningMove. One Five wins; otherwise an OpenFour dominates; two directions of
Four are DoubleFour; Four plus OpenThree is FourThree; two OpenThrees are
DoubleThree. Remaining single classes follow naturally. Closed Three does not
qualify for FourThree/DoubleThree. Each color is independent.

Profile semantics have no evaluator weights. A 4096-byte lookup table maps four
three-bit directional codes to a profile. All 6^4 valid combinations are checked
against the independent branch-based aggregation reference.

## Incremental PatternState

Root construction encodes every center once. PatternState owns:

- one occupancy bitboard;
- `[225][4]` u16 line keys, including keys at occupied centers;
- `[225]` four-byte DirectionSet values: four three-bit Black classes in bits
  0..11, four White classes in 16..27; all other bits zero;
- `[225][2]` one-byte profiles; occupied cells have canonical Quiet profiles;
- `[2][9]` u16 feature counts over empty cells only.

`LINE_INFLUENCES[225]` lists at most 32 `(center, direction, two-bit shift)`
updates. Non-center points on the four axes are disjoint, so no affected center
is visited twice. Each transition masks in the new field, looks up only that
changed direction, and updates its packed DirectionSet. If its class changes,
an empty center's profile is refreshed; counts change only if the profile did.
Playing a cell removes its own profile; undo restores it using its cached keys.
Occupied-center keys/classes are also maintained, so undo needs no line snapshot.

`PatternEvaluator::evaluate` reads nine counter differences, applies centralized
weights, chooses the side-to-move perspective, and clamps to +/-10,000,000. It
does not visit board cells or recognize patterns. Initial untuned weights are:

| Quiet | Three | OpenThree | DoubleThree | Four | FourThree | DoubleFour | OpenFour | WinningMove |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 | 20 | 200 | 2,000 | 10,000 | 50,000 | 100,000 | 120,000 | 1,000,000 |

These are features of hypothetical next placements, not counts of independent
forced wins; overlapping threats may contribute multiple times. No strength
optimality or equality with ClassicalEvaluator scores is claimed.

The test-only full-recompute oracle calculates geometry directly from Position,
independently of the precomputed influence lists. It compares every line key,
directional cache, profile, count, and score after every make/unmake. All empty
WinningMove flags are also compared to Core `would_win` for both colors.

## Transposition table

`AlphaBetaEngine` owns a contiguous `Vec<Bucket>`. Bucket count is a power of two,
the low key bits select a bucket, and each four-way bucket verifies the full
64-bit key before accepting a hit. The default 64 MiB table has 1,048,576
buckets and 4,194,304 entries.

An entry stores the full key, normalized `i32` score, optional packed best move,
`u8` depth, bound, and `u8` generation. `PackedMove` privately encodes validated
`Move.index() + 1` in `NonZeroU8`, so `Option<PackedMove>` uses Rust's safe niche
representation without a public sentinel. On the current supported target,
`TtEntry` is 16 bytes and a four-entry bucket is 64 bytes; regression tests pin
these cache-local dimensions without unsafe layout tricks.

Each public search advances one generation; all iterative depths within it share
that generation and table. The table persists between searches. On generation
counter exhaustion the u8 generation wraps without clearing. Replacement is
deterministic: same full key first, then empty slot, then greatest relative age
`current.wrapping_sub(stored)`, then shallower depth, then lower slot index.
After 256 generations age may alias; this affects eviction quality only, never
key/depth/bound correctness. A shallow/weaker update
does not trivially overwrite a deeper exact entry for the same key.

`AlphaBetaEngine::clear_transposition_table` provides an explicit cold-cache
reset for reproducible experiments; resize also starts empty. Capacity rounds down
to a power-of-two bucket count, with one bucket as the minimum (including 0 MiB).
The default remains 64 MiB. `transposition_table_statistics()` reports bytes,
buckets, entries, and sampled occupancy per mille by inspecting at most the first
1024 buckets (4096 entries), independent of total capacity. Occupancy includes old
generations and is approximate. Replacement counts record colliding full-key
evictions since explicit clear/resize; SearchStatistics records this search's
evictions separately. Same-key updates and empty-slot fills are not evictions.

### Bound correctness

Every Negamax node retains its original alpha and beta. A matching entry's legal
best move may influence ordering at any stored depth, but its score can affect
the result only when stored depth equals the requested remaining depth:

- `Exact` returns the stored score;
- `Lower` returns only when score is at least beta;
- `Upper` returns only when score is at most alpha;
- otherwise normal search continues.

The exact-depth restriction is deliberate: a deeper static minimax score is not
a mathematical bound on a shallower horizon. Accepting deeper entries caused a
warm ancestor search to report 19,380 where the cold depth-four search reported
-580. The regression is fixed without changing full-key checks, mate handling,
replacement, or legal-move reuse. This is stricter than the minimum sufficient-
depth condition and preserves fixed-depth semantics across public-search history.

After search, a result at or below original alpha is stored as `Upper`, a result
at or above beta as `Lower`, and an in-window result as `Exact`. Beta-cutoff
nodes therefore retain useful lower bounds. Root entries are stored exact after
root comparison.

### Mate-distance normalization

Search scores are always from the side-to-move perspective. `MATE_SCORE` is
100,000,000, static evaluation is clamped to +/-10,000,000, and finite search
infinity is 200,000,000. The mate threshold reserves the top 225 score points,
leaving a wide gap from ordinary evaluator values.

TT storage removes root-ply dependence: positive mate scores add the current
ply and negative mate scores subtract it. Probe performs the inverse adjustment
for its current ply. Faster wins therefore remain preferable and delayed losses
remain preferable even when the same position is reached at another depth.

## Move generation and ordering

Search calls `state.candidates()`, returning a fixed `[Move; 225]` plus length.
Move priorities use cached profiles and precomputed center bias. Ordered tiers:

1. own immediate win;
2. block opponent immediate win;
3. own OpenFour/DoubleFour/FourThree;
4. opponent equivalent dangers;
5. own Four/DoubleThree;
6. opponent Four/DoubleThree;
7. own OpenThree, then opponent OpenThree, then Three-like moves, then Quiet.

Each tier distinguishes its exact structural class. Within it, TT preference,
own/opponent profile, center bias, and lower canonical move index form the total
order. A packed u32 includes all these fields and the reversed move index.
Priorities are materialized in a fixed array, sorted allocation-free, then
converted through the validated Move table. Comparisons do no board reads,
`would_win`, geometry, or pattern recognition. Differential tests preserve the
previous lexicographic order exactly.

Root and exact ordinary-node comparisons explicitly prefer the smaller move
index on equal exact scores. If a root child only returns an alpha-bound equal to
the incumbent and has a smaller index, it is re-searched with a full window
before changing the canonical result. Thus a warm TT may reorder work but cannot
arbitrarily change the semantic root choice.

## Iterative Negamax, PV, and statistics

Search runs complete full-window iterations from depth 1 through
`SearchLimits::max_depth`. V0.3 has no interrupting limit, so a non-terminal
positive-depth search completes the requested nominal depth. All iterations
share the same TT and generation.

`SearchResult` distinguishes:

- `requested_depth`: the caller's maximum;
- `completed_depth`: the last fully completed iteration;
- `seldepth`: maximum ply visited anywhere in the public search;
- `principal_variation`: a legal searched prefix whose first move equals
  `best_move` when one exists.

A fixed 225-ply PV table is updated in place during recursion. It allocates no
`Vec` per node; the final public PV vector is constructed once. TT cutoffs may
shorten the reported line, so PV is guaranteed to be a valid prefix rather than
always the full nominal depth.

`SearchStatistics` counts all iterations in one public search: visited nodes,
static evaluations, beta cutoffs, TT probes, full-key hits, TT cutoffs, and
successful TT stores, and colliding replacements. Wall time is intentionally excluded from deterministic
engine statistics and measured externally by the benchmark utility.

## Determinism contract

Zobrist generation, candidate traversal, move ordering, replacement, and
equal-score selection are deterministic; no random source or unordered
collection participates. For identical semantic input, configuration, and
software version, semantic output means the same best move and score.

The persistent table is an optimization state. Warm and cold searches may have
different node, TT-hit, and wall-time measurements while retaining the same
semantic result. Reproducible performance experiments must use a fresh engine or
call the explicit table-clear method.

## Native adapter

The synchronous eframe/egui application draws the board, highlights the latest
move, maps clicks through validated `Move` construction, displays `GameStatus`,
supports side selection/New Game, and shows depth, seldepth, nodes, score, TT
statistics, and PV. It contains no win detection, move legality, evaluator, hash,
or search implementation.

## Explicit V0.3 non-goals

No aspiration windows, PVS, killer/history/continuation heuristics, LMR/LMP,
futility/null-move pruning, quiescence, VCF/VCT/TSS/DFPN, NNUE, MCTS, opening book,
randomization, time/node limits, cancellation, parallel/async search, server API,
AI arena, Renju, or Swap/Swap2. Core's backing storage stays at 225 cells.
Evaluator configuration can be shared immutably while each future worker owns
its own state, without introducing an unused worker/thread abstraction now.
