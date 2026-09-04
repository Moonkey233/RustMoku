# RustMoku V0.2 Architecture

RustMoku V0.2 extends the complete V0.1 vertical slice with stateful search
infrastructure. Domain rules remain independent of AI state, and the GUI remains
an adapter. The new hashing, cache, iterative-deepening, PV, and statistics code
is private to the engine unless it forms part of the public search boundary.

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

The intentionally small public surface is `Evaluator`, `ClassicalEvaluator`,
`SearchEngine`, `AlphaBetaEngine`, `EngineConfig`, `SearchLimits`,
`SearchResult`, and `SearchStatistics`. Candidate lists, ordering, hashes,
search-side state, TT entries, and PV tables remain private.

`SearchEngine::search` takes `&mut self` because `AlphaBetaEngine` owns a mutable,
persistent transposition table and generation counter. The caller still supplies
an immutable `&Position`, which search never mutates. No interior mutability,
synchronization primitive, background task, or global cache is involved.

## SearchState and Zobrist hashing

At the public search boundary, `SearchState` clones the caller's `Position`
exactly once and computes a full 64-bit key by scanning the 225 cells once.
Recursive search mutates this working position in place with make/unmake and
updates its key with O(1) XOR operations. There is no recursive position clone.

Zobrist data is generated deterministically at compile time from a fixed seed
using SplitMix64. Separate keys represent every `(Stone, Move)` occupancy,
Black/White side to move, and the Freestyle rule set. Exhaustive matches map
domain enums rather than relying on numeric discriminants. There is no runtime
random or mutable global state. Rule and side components are part of the key even
though V0.2 currently implements one ruleset.

The engine-private sidecar is the concrete extension point for future measured
incremental candidate or evaluation state; neither is implemented in V0.2.

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
counter exhaustion the table is cleared before reuse. Replacement is
deterministic: same full key first, then empty slot, then older generation, then
shallower depth, with slot index as the final tie-break. A shallow/weaker update
does not trivially overwrite a deeper exact entry for the same key.

`AlphaBetaEngine::clear_transposition_table` provides an explicit cold-cache
reset for reproducible experiments.

### Bound correctness

Every Negamax node retains its original alpha and beta. A matching entry's legal
best move may influence ordering at any stored depth, but its score can affect
the result only when stored depth is sufficient:

- `Exact` returns the stored score;
- `Lower` returns only when score is at least beta;
- `Upper` returns only when score is at most alpha;
- otherwise normal search continues.

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

An empty board generates only the center. Otherwise generation visits unique,
empty points within Chebyshev distance two of an existing stone. `MoveList` is a
fixed `[Move; 225]` plus logical length; unused slots contain a valid center move
and are never part of the logical slice. Generation uses a fixed marker array,
not a hash collection or heap allocation.

The logical slice is sorted in place with allocation-free `sort_unstable_by` and
a total deterministic order:

1. immediate win;
2. immediate opponent-win block;
3. TT move within the same tactical class;
4. local neighborhood/center heuristic;
5. ascending move index.

Root and exact ordinary-node comparisons explicitly prefer the smaller move
index on equal exact scores. If a root child only returns an alpha-bound equal to
the incumbent and has a smaller index, it is re-searched with a full window
before changing the canonical result. Thus a warm TT may reorder work but cannot
arbitrarily change the semantic root choice.

## Iterative Negamax, PV, and statistics

Search runs complete full-window iterations from depth 1 through
`SearchLimits::max_depth`. V0.2 has no interrupting limit, so a non-terminal
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
successful TT stores. Wall time is intentionally excluded from deterministic
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

## Explicit V0.2 non-goals

V0.2 does not implement aspiration windows, PVS, killer/history/continuation
heuristics, LMR/LMP, futility or null-move pruning, quiescence, VCF/VCT/TSS/DFPN,
bitboards, incremental candidate frontiers, incremental/broken-pattern
evaluation, NNUE, MCTS, opening books, randomization, time/node limits,
cancellation, multithreaded or async search, server APIs, AI arenas, Renju, or
Swap/Swap2. `ClassicalEvaluator` remains the stable V0.1 reference evaluator.
