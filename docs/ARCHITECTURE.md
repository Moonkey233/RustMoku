# RustMoku V0.9 Architecture

V0.9 builds on the V0.8 lifecycle, Arena and native adapter with CPU-only
multi-core Lazy SMP and a Safe-Rust shared ordinary transposition table.
Classical search remains the primary backend; this milestone adds no advanced
selectivity or learned evaluation.
Core remains authoritative for legality and wins; Native remains an adapter.
All first-party crates forbid unsafe code. Concurrency uses only the standard library.
Milestone scope and future work live in [ROADMAP.md](ROADMAP.md).

## Crate dependency graph

```text
rustmoku-engine -> rustmoku-core
rustmoku-native -> rustmoku-core
rustmoku-native -> rustmoku-engine
rustmoku-arena -> rustmoku-core + rustmoku-engine
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

Game owns a private Vec of played Move plus opaque MoveUndo, appended only after
a legal transition. `history()` is a read-only chronological Move iterator.
`undo()` consumes the last token; `undo_plies(n)` removes up to n plies and safely
stops at empty. Position, side, last move and winner are restored by Core undo;
the predecessor of every accepted game move was Ongoing, so status restores to
Ongoing. Tokens remain private record implementation data and are never exported.
History allocation occurs only for played/imported games, never in search nodes.

Move's Display/FromStr codec uses A..O (including I), rows 1..15 from bottom to top:
A1 maps to internal (14,0), O15 to (0,14), H8 to (7,7). Formatting is uppercase;
parsing accepts lowercase, rejecting out-of-range or malformed coordinates.
Internal coordinates, Zobrist and pattern geometry are unchanged. Every adapter
uses this codec; even board-axis labels derive from formatted Moves.

Records contain `RustMoku 1`, `rules=freestyle` and `moves=` followed by the complete
ordered sequence. Core's Game::from_record creates a fresh Game and calls
play_move for every token. Version/rules/syntax/coordinate errors and illegal
replay errors include the relevant line or ply. Formatting is deterministic with
a final newline. Native owns clipboard/editor/file I/O; Core has no filesystem
or UI dependency. Invalid import cannot mutate the current game.

The shared OPENINGS slice contains twelve hand-authored Freestyle test starts,
each with stable id/name, rules and validated Move sequence. Opening::game replays
through Game and can report a legality error; no direct Position writes exist.
There is no official balance provenance, RNG, balance score, D4 deduplication or
opening database. Native selection/cycling and Arena use the same stable order.

## Engine boundary and ownership

The intentionally small public surface includes `Evaluator`, `ClassicalEvaluator`,
`PatternEvaluator`, opaque `PatternState`,
`SearchEngine`, `AlphaBetaEngine`, `EngineConfig`, `TacticalConfig`, `ProofLimits`, `SearchLimits`,
`SearchResult`, `SearchInfo`, `SearchObserver`, `CancellationToken`,
`SearchTermination`, `TacticalProof`, `TacticalProofKind`, `SearchStatistics`, and
`TranspositionTableStatistics`. `PatternUndo` is private; the public PatternState
API debt is deferred to the NNUE/custom-evaluator milestone. Candidate lists,
ordering, hashes, search-side state, TT entries, and PV tables remain private.

`SearchEngine::search` takes `&mut self` because `AlphaBetaEngine` owns the
generation and the coordinator-owned VCF/VCT tables. The ordinary TT itself is
Safe-Rust shared state borrowed immutably by a temporary scoped Lazy-SMP team.
The caller still supplies an immutable `&Position`, which search never mutates.
No engine lock, global cache, or persistent internal thread pool is involved.
Native's worker owns its engine; the public-search shared state is limited to
the ordinary TT and one lifecycle control object.

## BoardState and SearchState: coordinated incremental ownership

```text
AlphaBetaEngine<E>                  evaluator configuration + ordinary TT + VCF/VCT solvers
  SearchState<E>                    one local working state per AB worker
    BoardState                     engine-private, evaluator-independent
      Position                     authoritative rules (one root clone)
      PositionKey                  incremental Zobrist key
      CandidateFrontier            occupancy/frontier bits and neighbor counts
      PatternState                 exactly one shared tactical state
    E::State                       evaluator-specific only (currently unit)
```

Normal recursion calls SearchState make/unmake, coordinating BoardUndo with
E::Undo. BoardState asks Core to accept each move before updating any sidecar;
rejected moves leave all state untouched. Undo consumes tokens in LIFO order.
No recursive Position or PatternState copies, dynamic dispatch, or locks exist.

`SearchState::prove_vcf` / `prove_vct` lend only the coordinator's private board
to the concrete solver and return an owned result after restoration. Ordinary
Lazy-SMP helpers construct their own root state after these stages. No mutable board getter or arbitrary
callback can expose a board/accumulator mismatch. The solver cannot access the
evaluator or E::State. Proven, NotProven, BudgetExceeded and outer-interrupted paths all unwind
board transitions; tests compare every field against a rebuilt board.

### Stateful evaluation

`Evaluator` has associated `State` and `Undo`, and `initialize`, `make_move`,
`unmake_move`, `evaluate` lifecycle methods. Its API is
`evaluate(&Position, &PatternState, &Self::State) -> i32`, from the side-to-move
perspective. Transition callbacks are infallible after Core accepts a move;
callers preserve the lifecycle and LIFO contract.

SearchState owns exactly one always-present `PatternState`, independently of the
evaluator. Ordering and qsearch read this same tactical state. Both current
evaluators have `State = ()`, `Undo = ()`: PatternEvaluator reads the shared
counts; ClassicalEvaluator ignores patterns and retains full reference scoring.
Future evaluators can own their own accumulator through the existing lifecycle.
On Windows x64, profile bitsets add 576 bytes: default SearchState is now 4,344
bytes (V0.4: 3,768), still with exactly one PatternState and unit evaluator state.

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
- `[2][9]` u16 feature counts over empty cells only;
- `[2][9]` BitBoard256 profile sets, with one membership per empty cell/color
  and no occupied-cell memberships. Transitions clear the old bit and set the
  new bit alongside profiles/counts. The existing recomputation oracle also
  verifies bitsets, counts, disjointness, and their union of all empty cells.

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

`AlphaBetaEngine` owns a contiguous `Vec<AtomicBucket>` plus a separate atomic
version sidecar. Bucket count is a power of two, the low key bits select a
bucket, and each four-way bucket verifies the full 64-bit key before accepting a
hit. The primary bucket remains four `AtomicSlot`s at 16 bytes each (64 bytes);
the sidecar adds one 8-byte version per bucket. The default 64 MiB primary table
has 1,048,576 buckets and 4,194,304 entries, plus 8 MiB of synchronization
storage, for 72 MiB of reported allocation.

An `AtomicSlot` stores the full key and one packed payload. The payload contains
the normalized `i32` score, optional packed best move, `u8` depth, bound and
`u8` generation. `PackedMove` privately encodes validated `Move.index() + 1` in
`NonZeroU8`, so no public sentinel enters Core. The logical `TtEntry` remains
16 bytes; primary atomic layout is checked by tests without unsafe code.

Writers claim a bucket's even version with AcqRel CAS and make it odd, choose
the same-key/empty/lowest-quality replacement slot, store that slot's key and
payload with Release, then publish the next even version with Release. A reader
loads the version with Acquire, misses while it is odd, reads every key and
payload with Acquire, and loads the version again with Acquire. It accepts only
equal even versions and a full requested-key match.

The first version Acquire makes the completed publication happen-before the
field loads, excluding older data. If either field comes from a later writer,
its Release/Acquire pair makes that writer's preceding odd claim happen-before
the final version load. Atomic coherence then prevents that load from returning
the old even version. Writers are serialized by the claim, so equality proves
one coherent bucket epoch even across same-key updates and colliding writers.
Relaxed field accesses do not establish this second edge: two Acquire version
loads alone are insufficient. The actual TT implementation has a Loom regression
that rejects the mixed key/score accepted by the original V0.9 protocol.

Each probe/store has bounded retries. At the last even u64 version, stores are
dropped until exclusive clear/resize; versions never wrap during shared access.
Clear requires `&mut self`, excluding diagnostic readers as well as workers, so
it can reset the contents and versions without an old snapshot surviving.

Each public search advances one generation; all iterative depths and workers
within it share that generation and table. The table persists between searches.
On generation counter exhaustion the u8 generation wraps without clearing.
Replacement is deterministic in single-thread mode: same full key first, then
empty slot, then lowest quality `depth + 4 * is_exact - 4 * age`, with lower
slot index breaking ties. Concurrent stores serialize per bucket only while
claiming the version; a racing store may be skipped, but replacement races do
not weaken key/depth/bound validation. After 256 generations age may alias; this
affects eviction quality only.

`AlphaBetaEngine::clear_transposition_table` provides an explicit cold-cache
reset for reproducible experiments; resize/reconfigure also starts empty and is
available only through the engine-owning mutable handle between searches.
`tt_memory_mib` describes primary bucket capacity. Capacity rounds down to a
power-of-two bucket count, with one bucket as the minimum (including 0 MiB), and
the default remains 64 MiB. `transposition_table_statistics()` reports primary,
sidecar and total bytes, buckets, entries, sampled occupancy per mille and
colliding replacement counts. Sampling inspects at most the first 1024 buckets;
occupancy includes old generations and is approximate. SearchStatistics records
the evictions observed by that public search separately. Same-key updates and
empty-slot fills are not evictions.

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
killer rank, history, own/opponent profile, center bias, and lower canonical
index form a packed u64 total order. Fixed-capacity arrays and integer-only
comparators avoid allocation and pattern calculation in sorting.

SearchHeuristics owns `history[Stone][Move]` and two distinct killers per ply.
Only beta-cutoff moves whose own and opponent profiles are both below Four are
learned. The history bonus is min(depth squared, 1024), using bounded gravity
below 16,384; killers keep the two latest distinct moves. Tactical tiers remain
above TT, history, and killers, regardless of previously learned values.

Root comparisons explicitly prefer the smaller move
index on equal exact scores. If a root child only returns an alpha-bound equal to
the incumbent and has a smaller index, it is re-searched with a full window
before changing the canonical result. Thus a warm TT may reorder work but cannot
arbitrarily change the semantic root choice.

## PVS, aspiration, PV, and statistics

The principal worker completes iterations from depth 1 to
`SearchLimits::max_depth`, sharing one TT generation with the helper workers.
Each worker owns a fresh `SearchHeuristics`, PV table and lifecycle handle. At
root and ordinary nodes, the first ordered child uses the full node window. Later
children use `[-alpha-1, -alpha]`; an improvement strictly below beta triggers
a full node-window re-search. Fail-high cuts off directly. Scores remain fail-soft
and TT bounds use the original alpha/beta. Root bound ties with a smaller index
are resolved by an infinity-window child search before replacing an exact best.

Depth >= 2 starts at previous score +/- 10,000. Fail-low/high doubles the delta
until a result falls strictly inside the window, capped at full search infinity.
Previous or newly discovered mate scores bypass further narrow windows. Finite
infinity and saturating endpoint arithmetic prevent overflow. Interrupted retries
propagate an explicit stop and never publish their partial score or PV.

### Exact immediate tactics

Root, Negamax, and qsearch check terminal state, then share `immediate_tactic`.
It reads cached winning bitsets and at most two set bits, never scans the board:

1. Own winning point: choose the lowest index and return `MATE - (ply + 1)`.
2. No own win and one enemy winning point: search only that forced block.
3. No own win and multiple enemy winning points: return `-MATE + (ply + 2)`.
   Every legal move loses in two. Prefer meaningful resistance: block the lowest
   enemy winning point, then use the next canonical winning point as the terminal
   reply. More than two threats still select the first two points. This changes
   neither score nor distance and does not invoke Alpha-Beta.

Own wins take priority because the game ends before any opponent reply. These
facts bypass TT scores; direct results are not stored because the cached bitset
query is already cheap and independent of nominal depth. In normal Negamax,
forced blocks allow full-key, equal-depth Exact/Lower/Upper TT score cutoffs with
mate normalization. If search continues, the candidate list remains exactly the
block; an unrelated legal hash move cannot replace it. Exact proof prefixes update
seldepth without counting unvisited nodes. Qsearch never uses ordinary TT scores.

### Threat quiescence

Normal depth zero enters qsearch without ordinary TT score probes/stores. After
terminal/immediate checks, a unique forced block recurses even at the cap. With
no immediate obligation, static stand pat is always available, including when
the opponent has potential Four/FourThree/DoubleFour/OpenFour points. The latter
are possible future threats, not a current mandatory defense.

After stand-pat beta cutoff and alpha update, noisy moves are generated directly
as `CandidateFrontier bits & own profile bits >= Four`, then ascending set-bit
iteration materializes a fixed-capacity ordered list. No scan of ordinary
candidates, ordinary Three/OpenThree/DoubleThree expansion, or optional enemy
non-immediate defensive points is included in V0.9.

The expansion cap remains six qplies. At/after it, only exact immediate facts and
forced-block chains continue; each reply fills a cell, so the 225-cell board is
the absolute bound. Quiet or non-immediate states at the cap return static
evaluation. A zero nominal-depth public call reports qsearch score/statistics
but keeps best_move=None and an empty public PV.

### Conservative LMR

Only normal non-root scout nodes (beta = alpha + 1) may reduce. Remaining depth
must be at least 3, move index in the ordered list at least 8 (ninth move), both
profiles exactly Quiet, history below 128, and killer rank zero. Hash moves,
forced blocks, PV windows, and mate-range windows are excluded. Reduction is
always one ply; no two-ply policy or reduction table is introduced.

The reduced child uses a null window. A score above alpha must repeat the normal
PVS path at full nominal depth; a reduced fail-low never updates alpha or PV.
This is a heuristic decision, not a mathematical proof about full-depth minimax.
The returned `NodeResult { score, validity }` carries two directional flags.
Negation swaps lower/upper evidence. A maximum's upper bound needs every relevant
child's verified upper bound; its lower bound needs a verified child attaining
that score. Scout/TT/qsearch bounds expose only their established direction;
scout equality cannot repair missing lower evidence. Unsearched cutoff siblings
prevent upper validity. Statistics only observe work.

An unverified reduced fail-low removes upper evidence but cannot invalidate a
later nominal-depth child proving beta cutoff. That Lower bound can be stored;
Exact requires both directions and Upper requires all necessary child evidence.
A nominal retry replaces the discarded reduction. Reduced children may cache
valid bounds at their actual depth. Equal-depth ordinary TT probes and mate
normalization are unchanged.

## Exact continuous-four (VCF) proofs

The private VcfSolver fixes an attacker Stone and operates only on BoardState.
It returns ProvenWin, NotProven, BudgetExceeded, or Interrupted. NotProven means
no proof within the continuous-four definition and max proof plies; it never
means lost. Interrupted is reserved for an outer cancellation, deadline, or
global work stop and is never cached as a disproof.

Terminal state comes first. At an attacker turn an immediate win ends the proof.
Otherwise, ascending Four-or-stronger profile bitsets generate attacks without a
board scan. Every resulting defender node rechecks actual tactical facts:

1. A defender immediate win refutes the line, including attacks making an open four.
2. At least two distinct attacker winning cells prove a win in two plies.
3. Exactly one attacker winning cell permits only that forced block.
4. Zero attacker winning cells rejects the branch: the continuous four stopped.

Thus an attacker facing an enemy winning point must remove it while creating a
four, unless the attack itself ends the game. There are no heuristic defenses.
Qsearch and VCF share `immediate_tactic` and `forcing_moves`; qsearch retains
bounded static evaluation while VCF only proves terminal wins.

Nonterminal attacker depths are 1,3,5,...; defender depths are 2,4,6,... .
Zero is reserved for already-terminal facts, so impossible parities cost no nodes.
Ascending move order at every attack selects the canonical smaller index among
equal-length proofs. The first successful iteration is shortest within VCF.
The PV includes the final attacker win, using a canonical blocking defender reply
for the double-winning-cell fact. Root score is MATE_SCORE minus terminal plies
(root absolute ply is zero); proof scores never enter the ordinary TT.

### Proof cache and deterministic limits

The dedicated table allocates 4096 contiguous four-way buckets once: 16,384
entries, 24 bytes each, 96 bytes per bucket, 393,216 bytes (384 KiB) on the
supported target. Entries contain the full attacker-salted PositionKey, u64
generation, searched depth, NotProven/ProvenWin status, optional validated move,
and proven distance. BudgetExceeded has no cache representation.

A deeper complete NotProven can answer a shallower request. A proven P-ply entry
is usable whenever P <= requested remaining depth, independently of the depth
where it was first found. It reconstructs a complete certificate through cached moves
and current tactical facts. Every move and forcing obligation is validated; a
missing/evicted descendant falls back to search, never to a truncated proof PV.
Replacement chooses the same key, then the first stale slot, then the shallowest
current entry with lower slot index breaking ties. Deeper same-key entries survive
shallower writes. There is no cache-based attack reordering.

Each public search advances the proof table generation and resets one dedicated
remaining-node budget. Probes accept only the current generation, including all
proof iterations in that public call. Old public searches cannot help a new call
cross its deterministic budget. No per-search table clearing occurs; only u64
generation exhaustion clears before restarting at one, preventing stale aliases.

EngineConfig defaults to 11 proof plies and 2,000 proof nodes. Each visited proof
node costs one, including depth-zero nodes and accepted cache hits. Bounded
certificate replay has no branches and costs no local expansion nodes (at most
the proof depth transitions). V0.9 charges each replay visit to global work. Depth is
also clamped to remaining board cells. Zero in either config field disables the
root attempt. The shared lifecycle independently enforces outer work/time/cancel limits.

The engine calls VCF after exact root facts and before VCT/iterative deepening, only for nonterminal,
positive nominal-depth roots with own Four+ candidates. NotProven/BudgetExceeded
continues existing search. ProvenWin returns exact proof metadata and terminal
PV with completed_depth=0 and seldepth=proof plies. Zero nominal-depth calls
retain analysis-only qsearch behavior. No normal-node or qsearch VCF probes run.

`vcf_nodes`, `vcf_cache_hits`, `vcf_probes` (gated solver attempts), `vcf_proven`,
and `vcf_budget_exhausted` report proof work separately from Alpha-Beta nodes/qnodes.

`SearchResult` distinguishes:

- `requested_depth`: the caller's maximum;
- `completed_depth`: the last fully completed iteration;
- `seldepth`: maximum ply visited or resolved in an immediate proof prefix;
- `tactical_proof`: optional VCF/VCT kind/distance, with no completed nominal iteration;
- `principal_variation`: a legal searched prefix whose first move equals
  `best_move` when one exists.

A fixed 225-ply PV table is updated in place during recursion. It allocates no
`Vec` per node; owned public PV snapshots allocate only at completed root events. TT cutoffs may
shorten the reported line, so PV is guaranteed to be a valid prefix rather than
always the full nominal depth.

`SearchStatistics` counts all Alpha-Beta iterations in one public search: visited nodes,
qnodes (a subset of nodes), PVS/tie re-searches, LMR reductions/full-depth retries,
aspiration fail-low/high retries, static evaluations, beta cutoffs, TT probes,
full-key hits, TT cutoffs, successful TT stores, and colliding replacements.
`worker_count`, `principal_nodes` and `helper_nodes` expose compact parallel
observability; the final result aggregates all joined workers, while an emitted
`SearchInfo` is the principal worker's completed-depth snapshot. Seldepth may
exceed nominal depth plus six through exact immediate tactics. Wall time is
intentionally excluded from deterministic engine statistics and measured
externally by the benchmark utility.

## Exact threat-space VCT and DFPN

`vct/threat.rs` defines a Copy, engine-private `ThreatDescriptor`: gain move,
forcing kind (the seven OpenThree-or-higher ThreatProfile variants), continuation,
defense, and dependency BitBoard256 sets. Quiet and ordinary Three cannot construct
one. A descriptor belongs to the defender node immediately after its gain;
responses always use BoardState make/unmake and subsequent attacker nodes rebuild
all tactics on the resulting board. No virtual simultaneous defenses are used.

### Separate tactical metadata

The slow semantic classifier and build script simulate every LineKey/color and
emit `threat_meta.bin`: 65,536 keys x two colors x four bytes = 512 KiB, contiguous
and aligned to 64 bytes. Each record contains directional kind plus three u8
masks for the eight non-center cells. Four continuations win immediately;
OpenThree continuations create two distinct winning cells. Dependencies union
all actual five-window witnesses for those continuations, including supporting
stones; costs include their empty cells. These conservative costs may include a
cell that damages one route while another survives. Broken and wall shapes use
the same simulation, never handwritten strings. Runtime lookup is O(1).

The 128 KiB PatternPair table remains exactly two bytes per key. Normal ordering,
evaluation, and make/unmake do not touch the tactical table. PatternState adds
only a crate-private line-key accessor, with no new hot state. An exhaustive test
checks all 131,072 tactical records against slow simulation; fixture audits also
check their actual-board implications independently.

### Defender-response soundness

Immediate facts always precede non-immediate threat handling: a defender winning
move refutes, two attacker winning points prove defeat in two, and one attacker
winning point restricts to its unique forced block. Attacker own wins take
priority; an attacker under an immediate obligation must block while forcing.

Otherwise responses are direct cost cells UNION defender Four-or-stronger moves.
An omitted legal move cannot occupy any empty five-window witness cell and cannot
create a defender winning point: any move creating such a point has at least a
Four profile. Thus an OpenThree continuation remains legal and creates two
attacker winning points. Adding an attacker stone cannot create a defender win.
This supplies an actual three-ply attacker win after every omitted reply.
Because there was no attacker winning point before that reply, adding a defender
stone cannot create one, so the three-ply distance is also a lower bound.

One lowest-index omitted move is included as a representative. Every omitted
reply has the same exact distance three; the representative preserves the
canonical slowest-defense choice when direct responses also lose that quickly.
If direct responses require longer proofs they dominate the representative.
Every listed response is searched on the real board; old OpenThree assumptions
are never carried through a defense. A test-only all-legal scan audits dependency
preservation and lack of counter-wins for omitted moves on bounded fixtures.
A separate shallow minimax oracle enumerates every legal defender reply and uses
Core immediate-win detection to compare proof status, distance, and canonical PV.
The differential regression generates 48 deterministic candidates and filters tactical states
from rotated/reflected/shifted seeds plus legal perturbations, for both attacker
colors and caps 3/5. Production and the all-legal oracle agree exactly on bounded
proof status, distance and canonical PV; no counterexample was found. An omitted
quiet response changes no active dependency and creates no defender Four+, so
the preserved forcing continuation wins before that stone changes initiative.
Production never scans all legal defender moves.

### DFPN traversal and cache

`vct/dfpn.rs` independently implements standard most-proving-child DFPN:
OR pn=min(child pn), dn=saturating sum(child dn); AND reverses min/sum.
The selected child's min threshold is limited by second-best+1 and the parent's
threshold; its sum threshold subtracts sibling contributions. Equal priorities
choose the lowest move index. u32 proof/disproof numbers saturate at u32::MAX/2;
only zero denotes solved. Numerical saturation without a proof stays Unknown.
Fixed move/number arrays bound every recursive frame; there is no per-node heap
allocation, Position clone, dynamic dispatch, synchronization, or unsafe code.

`vct/table.rs` owns a dedicated four-way fixed bucket table. On Windows x64 each
entry is 48 bytes and a bucket 192 bytes. Entries verify full PositionKey,
attacker, node phase, 64-bit deterministic descriptor signature, exact remaining
depth, and u64 public-search generation. The signature mixes gain, kind, and all
four words of every mask; distinct active obligations do not share an AND entry
merely because board and attacker match. As with Zobrist, signatures are hashes.
Entries carry pn/dn, solved state encoded by zero, optional legal best move, and
an optional exact distance populated only after canonical reconstruction.

A 16 MiB request rounds down to 65,536 buckets / 262,144 entries / 12 MiB. This
holds substantially more AND/OR work than the 384 KiB VCF table without changing
ordinary TT layout. Replacement uses same key/depth, then stale generation,
then unsolved/shallow entries, ties by slot. A one-bucket minimum remains correct.
Probes never reuse a different proof depth or an old public generation. A fresh
UNKNOWN write cannot replace useful same-key/same-depth partial numbers or a
best move; solved entries stay protected. No proof-number merge is attempted.
Attacker bucket salt uses an exhaustive Stone mapping. Generation wrap clears
before reuse. Interrupted work cannot become a solved disproof.
Evicted certificates are reconstructed under budget; no truncated proof is emitted.

### Distance, budget, and root integration

DFPN alone proves existence within a cap, not the shortest distance. A canonical
reconstruction pass searches parity-aware increasing limits at each node. OR
selects its first proven attack at the shortest limit. AND reconstructs every
response at its own shortest limit, selecting maximum child distance. Both add
one ply and break equal-distance ties by lower Move index. The resulting PV
follows fastest canonical attack and slowest canonical defense to an attacker win.
The distance is exact within the V0.7 forcing vocabulary, not unrestricted Gomoku.

ProofLimits/TacticalConfig group VCF and VCT budgets and VCT table memory.
Defaults retain 11/2,000 for VCF and use 9/4,000 for VCT. Depth is clamped by empty
cells. VCT charges every node inspection, including child initialization, cache
hits, and certificate visits, to one dedicated remaining-node budget. Only fixed
immediate prefixes and final PV copying are outside that accounting. VCF retains
its locally uncharged, nonbranching certificate replay, which does consume global
work. Neither limit counts individual CPU instructions. Statistics never determine
validity.

Positive-depth root order is exact immediate facts, VCF, enabled VCT with an own
OpenThree-or-stronger candidate, then normal Alpha-Beta. Exact immediate roots
return completed_depth=0 without invoking either solver. VCT ProvenWin returns
kind=Vct, MATE_SCORE-plies, completed_depth=0, seldepth=plies, and a full proof PV.
NoProof/BudgetExceeded falls through. No DFPN calls occur inside Alpha-Beta or
qsearch. Zero nominal-depth analysis remains unchanged.

Only `vct_nodes`, `vct_cache_hits`, `vct_proven`, and `vct_budget_exhausted` are added.
NoProof means no proof within the chosen vocabulary/depth, never a loss verdict.

## Determinism contract

Zobrist generation, candidate traversal, move ordering, replacement, and
equal-score selection are deterministic; no random source or unordered
collection participates. With `EngineConfig::threads() == 1`, identical
semantic input, configuration and software version preserve the strict V0.8
reference result (best move and score). The library default is one worker.

With more than one worker, the root rotation is deterministic and no RNG or
evaluator noise is introduced, but helper/principal interleaving changes shared
TT timing and heuristic work. Parallel searches therefore may produce different
work, wall time or heuristic result; they are not promised bit-for-bit stable.

The persistent ordinary TT is an optimization state. In the one-worker reference
mode, warm and cold searches may have different node, TT-hit, and wall-time
measurements while retaining the same semantic result. Reproducible performance
experiments must use a fresh engine or call the explicit table-clear method.

## Search lifecycle and completed snapshots

`SearchLimits::new(max_depth)` retains convenient fixed-depth search. Builders
`with_max_nodes(u64)` and `with_move_time(Duration)` set optional per-public-search
limits. Both apply to root tactical solving and normal iterations. Core contains
no clock, budget, channel or scheduling state. `SearchEngine::search_controlled`
accepts the same Position/limits plus an owned cancellation token and a borrowed
`dyn SearchObserver`; the ordinary `search` supplies a fresh token and no-op
observer. This contract is independent of Alpha-Beta and can serve future backends.

`CancellationToken` is a cloneable `Arc<AtomicBool>`, using Relaxed loads/stores:
it transports no other data. Cancellation is one-way; there is no reset or token
reuse protocol. One public search creates a shared control object containing the
token, start time, optional elapsed Duration, optional global node cap, latched
public stop reason and internal team-done flag. Each AB worker then receives a
local `SearchBudget` handle with its own work counter and the shared control.
VCF/VCT use the coordinator's handle. Uncapped ordinary nodes therefore avoid a
contended atomic RMW; capped searches use exact shared admission. No Arc clones
occur in recursion and no engine Mutex/RwLock exists.

One work unit admits one normal node (qnodes included once), VCF visit, VCF
certificate replay visit, or VCT/DFPN inspection/certificate visit. VCT child
initialization and accepted cache hits consume work. Fixed immediate PV prefixes,
root state construction, static fallback evaluation and snapshot copies are not
extra nodes. Exact root facts count as one admitted normal visit when possible;
if admission is already stopped their known exact answer remains usable without
exceeding the cap. Existing subsystem counters remain separate; global work can
exceed nodes + vcf_nodes + vct_nodes because VCF replay is locally uncharged.

The integer cap is checked exactly by shared admission: N admits at most N visits
across root proofs, worker 0 and every helper. The atomic and optional clock are
polled before the first charge, every 256 local charges, before root solvers,
between solver stages/depths, and before publishing an iteration/proof. Polls do
not charge nodes. Reaching the node count exactly at the requested final result
can still mean Completed; NodeLimit means additional work was denied.
Cancellation takes precedence over time at a poll, and the first observed public
reason is latched. When the principal worker finishes the requested depth it
marks team-done; helper shutdown is internal and cannot turn that completed result
into Cancelled. Local proof exhaustion means BudgetExceeded and may fall through;
an outer stop is a distinct Interrupted/Stopped result, never a cached
NoProof/NotProven. A caller's slow evaluator/observer can delay polling; these
are cooperative deadlines, not hard real-time guarantees.

Alpha-Beta and qsearch return `Result<_, Stopped>`. Child re-searches are enclosed
in a fallible scope, then Position, hash, frontier, PatternState and evaluator
undo are restored before `?` propagates. Proof recursion uses the same
restore-before-propagate rule, including interrupted VCF cached certificate
replay. No panic/unwind cancellation is used. Interrupted ancestors perform no
TT stores; valid completed descendants may retain their normal bounds.

`SearchTermination` is Completed, NodeLimit, TimeLimit or Cancelled. A stopped
iteration, including any aspiration retry, cannot replace the last completed
move, score, PV, nominal depth or seldepth. Final statistics include all work,
including a discarded iteration; SearchInfo statistics reflect the completion
instant. Before any nominal iteration completes, positive-depth fallback is the
lowest candidate index (center if empty), static side-to-move score and one-move
PV, with completed_depth=0. It is not a completed minimax evaluation. Zero-depth
analysis keeps no move/PV and uses its completed qsearch score if available.
Immediate exact facts remain valid; VCF/VCT certificates are accepted only after
complete reconstruction and a boundary poll, with tactical_proof and nominal
depth zero.

`SearchInfo` owns completed depth, seldepth, best move, root-side score, PV,
statistics and optional tactical proof. An observer receives snapshots only after
completed depths or exact tactical root events, never per node. It may send them
over channels; engine callers are not required to manage channels. The observer
must return promptly, since its elapsed time belongs to this search. Warm ordinary
TT history can affect which depths fit a node/time limit: fixed-depth semantic
determinism does not promise identical interrupted output across different cache
histories. Fresh engines and node limits provide reproducible Arena experiments.

## Native adapter and worker ownership

The eframe/egui UI draws public Game/Position state, translates validated Moves,
and displays completed depth, seldepth, total work, qnodes, worker count, score,
TT statistics, PV and proof distance. Depth (1..12), thread count and ordinary
TT MiB apply to the next request; optional move time (0 = unlimited) remains a
per-request limit. No search, rules or evaluator logic lives in presentation.

One persistent standard-library thread constructs and exclusively owns its
AlphaBetaEngine. An mpsc request carries a monotonically increasing u64 ID, owned
Position snapshot, limits and fresh token. During one request the engine
temporarily scopes its principal plus helper Alpha-Beta workers; the outer Native
worker remains persistent. A separate mpsc channel returns tagged SearchInfo and
SearchResult events. Request payloads are boxed once off the hot path; channels
never block the search on UI consumption, and events are coarse. Ordinary TT
allocation/history survives consecutive moves unless the user changes TT MiB,
which is sent as an owner-thread reconfiguration and starts an empty table.

The UI-side SearchWorker owns the matching token clone and current ID. New Game,
side change, Undo, successful import, a human move starting a request and shutdown cancel the old token
and advance the ID without wrapping. One admission gate rejects every stale ID
and any duplicate final event after the active request ends. Reconfiguring
Threads or TT MiB first cancels/invalidates the active ID, sends a FIFO command
to the owning thread, and starts a replacement search only when the game still
needs one. Only a current,
non-cancelled result can call Game::play_move, with an ongoing game and AI turn.
Input cannot enqueue duplicate AI moves. While searching, UI frames poll events
and request repaint every 16 ms; New Game invalidates immediately, regardless
of when the old worker search notices cancellation. Drop cancels, sends Shutdown
and joins; no engine lock, async runtime or detached thread exists.

### Native local-game session and stable layout

The UI stores an undo_floor separate from Game/Position. Selecting an opening
sets it to that replayed history length; importing a record starts with floor
zero. Human-decision Undo locates the last human stone after the floor and undoes
it and subsequent replies. A pending AI reply means one ply is removed; a finished
reply normally means two. Terminal states use the same rule. There is no prior
human decision behind an initial AI move, so Undo is disabled there. The UI
cancels/advances its ID before mutation, clears displayed search state, and checks
whether the restored side requires a new AI request. Game edits do not clear or
resize the worker's ordinary TT.

A fixed 238-point top controls panel and fixed 180-point history panel isolate
the remaining board rectangle. PV is one horizontal scroll row, and history is
vertically bounded. Live proof/PV/error text cannot resize the board. Board labels
occupy existing margins; click geometry stays shared with stone rendering. Move
numbers are an optional overlay derived from chronological Game history, and the
last-move marker remains. A separate record editor window supports canonical
export/copy, legal import and explicit standard-library file load/save.

## Headless Arena

`rustmoku-arena` depends only on Core and Engine. It runs matches sequentially and
replays one of twelve shared fixed legal opening prefixes through Game for each
paired leg: A is Black then White on the same board. Both engines are fresh per
game; their ordinary TTs persist between that game's moves. Player-specific
EngineConfig includes an independent Alpha-Beta thread count, TT size and
Pattern/Classical evaluator selection; a move may temporarily fan out its own
Lazy-SMP team. Common positive depth and optional global node limits apply per
move. The board fills or Core declares a winner; there is no heuristic
adjudication or arbitrary move cutoff.

CSV records pair/opening id/leg, A color, winner, plies, searched moves and total work.
Configuration and summary go to stderr. A win earns one point, a draw half; A's
paired score is reported out of two points per pair, alongside A/B wins, draws
and average work per searched move. Identical version, opening order, player
configs and limits reproduce the one-worker reference run. Multi-worker matches
are useful for scaling/strength experiments and may be schedule-dependent. There
are no random openings, parallel matches, Elo estimates or tournament
infrastructure.

## Explicit V0.9 non-goals

No advanced selective pruning, advanced qsearch redesign, interior VCF/VCT,
NNUE, policy networks, SIMD optimization, unsafe code, MCTS, AlphaZero,
Transformer evaluation, GPU compute, opening database, server/protocol layer,
full-game clocks, SPRT/Elo framework, Renju, Swap/Swap2, or a generic persistent
thread pool. Core's backing storage remains 225 cells. Future scope is in
[ROADMAP.md](ROADMAP.md).
