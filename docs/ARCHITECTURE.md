# RustMoku V0.1 Architecture

RustMoku V0.1 is a vertical slice from domain rules through deterministic search
to a native desktop adapter. Its boundaries are intentionally small: extension
points are documented where an interface already has a current user, while
future subsystems are not scaffolded in advance.

## Crate dependency graph

```text
rustmoku-core
    ^       ^
    |       |
rustmoku-engine
    ^
    |
rustmoku-native
```

More precisely:

```text
rustmoku-engine -> rustmoku-core
rustmoku-native -> rustmoku-core
rustmoku-native -> rustmoku-engine
```

`rustmoku-core` has no third-party dependencies and owns all Gomoku semantics.
`rustmoku-engine` has no presentation dependency. `rustmoku-native` is an adapter:
it draws public state, maps pointer coordinates to validated `Move` values, calls
`Game::play_move`, and invokes `SearchEngine` with `Game::position()`.

## Core domain

### Move invariants

`Move` is a compact newtype with a private `u8` representation. The only public
constructors validate a row/column pair or an index, so values 225 through 255
cannot enter the domain. No sentinel move exists; move absence is `Option<Move>`.
`Move::all()` enumerates exactly the 225 legal board coordinates and
`Move::CENTER` identifies the center intersection.

### Position invariants

`Position` owns exactly 225 `Option<Stone>` cells plus the side to move, `RuleSet`,
move count, and last move. All fields are private. Read-only access is exposed by
value or immutable reference, and callers cannot obtain mutable access to the
cell array.

A successfully constructed or transitioned position maintains:

- every stored move is on the board;
- move count equals board occupancy;
- Black moves first and the side changes after every move;
- `last_move` is `None` exactly for the initial position and otherwise points to
  the most recently placed stone;
- occupied intersections and terminal positions reject further moves;
- the active `RuleSet` travels with the position consumed by search.

`Stone` contains only `Black` and `White`; emptiness is represented independently
as `Option<Stone>`.

### Make/unmake model

`Position::make_move` is the sole forward mutation operation. It returns an
opaque, non-cloneable `MoveUndo` containing the previous state required for an
exact reversal. `Position::unmake_move` consumes this token and requires LIFO
order. It checks token/state agreement before mutation. Search creates one
working `Position` clone at its root, then all recursive nodes reuse it through
make/unmake; recursive position cloning is forbidden.

### Rules and game flow

`RuleSet` is currently a one-variant enum, `Freestyle`. A line of five or more
contiguous stones wins. Win detection begins at `last_move` and counts in both
directions along horizontal, vertical, backslash-diagonal, and slash-diagonal
axes. It does not rescan the full board.

Opening protocols are not board rules and are not represented by `RuleSet`.
`Game` wraps a `Position` and owns actual-play status (`Ongoing`, `Won`, or
`Draw`). Search accepts `&Position`, never `Game` or UI state.

## Engine

### Evaluation

`Evaluator` is the narrow engine boundary used today by `AlphaBetaEngine`.
`ClassicalEvaluator` scans all four line directions and scores contiguous FIVE,
OPEN_FOUR, CLOSED_FOUR, OPEN_THREE, CLOSED_THREE, OPEN_TWO, and CLOSED_TWO runs.
Weights remain private implementation details.

Every evaluator score is from the side-to-move perspective: positive favors the
player about to move and negative favors the opponent.

V0.1 evaluation deliberately does not recognize broken threes, broken fours,
compound threats, or deeper shape interactions. It is a correctness-oriented
baseline, not a complete Gomoku pattern evaluator.

### Candidate generation and ordering

An empty position generates only `Move::CENTER`. Otherwise candidates are unique,
legal empty cells within Chebyshev distance two of any stone. Generation uses a
fixed 225-entry boolean marker array and a fixed-capacity `MoveList`; it uses no
`HashSet` and performs no heap allocation.

Ordering classifies immediate wins first, blocks of an opponent's immediate win
second, and ordinary moves third. A small neighborhood/center heuristic orders
moves inside each class. Ascending move index is the final tie-break, making the
entire ordering deterministic.

### Search semantics

`AlphaBetaEngine` directly implements fixed-depth Negamax with fail-soft
Alpha-Beta pruning. It does not contain evaluator weights. The public boundary is:

- `SearchLimits`: requested fixed maximum depth only;
- `SearchEngine`: immutable `&Position` input;
- `SearchResult`: optional best move, score, requested/reached depth, and node
  count.

Terminal scores use finite constants separated from static evaluation and from
search infinity. Mate distance is included in terminal values, so shorter forced
wins score higher and longer forced losses score higher than immediate losses.
No arithmetic uses `i32::MAX` or `i32::MIN` as infinity.

For a fixed position, limits, and software version, search is deterministic. It
uses no random source, implicit seed, time cutoff, concurrency, or unordered hash
collection.

## Native application

The eframe/egui application draws the board and stones, highlights the last move,
maps clicks to `Move::from_row_col`, displays `GameStatus`, lets the human select
Black or White, starts a new game, and displays the last AI search's depth, node
count, and score. When the human selects White, a new game synchronously asks the
engine to play Black's first move.

The UI does not detect wins, decide legality, inspect evaluator internals, or
mutate `Position`. V0.1 search runs synchronously on the UI thread by design.

## Explicit V0.1 non-goals

V0.1 contains no Zobrist hashing, transposition table, iterative deepening,
aspiration windows, PVS, killer/history heuristic, LMR, quiescence search,
VCF/VCT/TSS/DFPN, NNUE, MCTS, neural runtime, opening book, random play,
multithreaded search, async runtime, cancellation, time control, streaming,
AI-vs-AI arena, persistence, position import/export, undo UI, Renju, Standard
Gomoku, forbidden moves, Swap/Swap2, or server API.

## Real future extension points

The existing `RuleSet` enum can gain separately tested board-rule variants.
`Evaluator` permits another concrete evaluator to be selected without changing
Negamax. `Position` mutation already centralizes future incremental caches behind
make/unmake. Search limits and results form the boundary where later, concrete
milestones can add capabilities. None of those future implementations are
present in V0.1.
