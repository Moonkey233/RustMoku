# RustMoku Roadmap

This file is the repository source of truth for milestone scope. Future items
are plans, not implemented capabilities or permission to expand a current task.

## Done: V0.1–V0.5 foundations

- Safe Freestyle Core, validated moves, reversible Position, native adapter.
- Replaceable evaluation; classical reference and incremental pattern evaluator.
- Deterministic hashes, fixed-size TT, candidate frontier and profile bitsets.
- Fail-soft PVS, aspiration iterative deepening, canonical root ties, mate scores.
- Exact immediate tactics, bounded threat qsearch, history/killers, conservative LMR.

## V0.6: Exact VCF (implemented)

- Result-owned LMR completeness and valid TT scores at forced-block nodes.
- Private BoardState separated from evaluator-specific SearchState data.
- Exact Freestyle continuous-four proofs with shortest distance and canonical ties.
- Dedicated generation-scoped proof table and deterministic node/ply budgets.
- Gated root integration, proof metadata, legal terminal PV, native proof display.
- Focused correctness tests and the lean fixed-position performance check.

## V0.7: Exact VCT / Threat-Space / DFPN (implemented)

- Bound-aware LMR/TT validity; parity-aware VCF and validated wider-depth reuse.
- Private threat descriptors with simulated continuation, cost, and dependency masks.
- Separate build-generated tactical metadata; the normal pattern table stays two bytes.
- Exact defender obligations, Four+ counter-threats, and all-legal omission audits.
- Bounded saturating DFPN with a dedicated context/depth/generation-sensitive table.
- Shortest attack / longest defense distance reconstruction and canonical terminal PV.
- Grouped proof configuration; gated root integration after immediate facts and VCF.
- Shallow independent AND/OR oracle, focused regressions, and six lean benchmarks.

## V0.8: Search Lifecycle, Arena & Async Native (implemented)

- Bounded generated VCT defender differential audit and partial DFPN entry protection.
- Per-move depth / total-work / Duration limits and one-way cancellation tokens.
- Explicit stop reasons; interruption preserves the last completed iteration.
- One budget across Alpha-Beta, qsearch, VCF and VCT, with coarse clock polling.
- Completed-only SearchInfo snapshots through an application-independent observer.
- Single-threaded Arena, paired fixed openings and configurable player engines.
- Persistent native worker owns the engine; stale request IDs are rejected.
- Clean cancellation/shutdown/join and responsive native controls.
- Game-owned move history, generic LIFO undo and human-decision session undo floors.
- Shared human notation and versioned records imported only by legal replay.
- Twelve shared hand-authored openings, deterministic selection and paired colors.
- Stable Native board/PV/history layout, coordinates and optional move numbers.
- Exact known-loss resistance at canonical opponent threat points.
- Time management and application scheduling remain outside Core Position.

## V0.9: Multi-core Lazy SMP / shared TT (implemented)

- CPU-only Lazy SMP with one authoritative principal worker and independent helpers.
- `EngineConfig::threads()` defaults to one and has no library-imposed maximum.
- Shared ordinary four-way TT using atomic key/payload slots and bucket seqlocks.
- Full-key validation, packed atomic payloads, deterministic single-thread replacement,
  and documented synchronization-sidecar memory accounting.
- One global capped-work admission counter across tactical and all AB workers;
  uncapped searches retain worker-local hot-path counters and aggregate after join.
- VCF/VCT remain coordinator-owned root stages and are never duplicated by helpers.
- Native Threads/TT MiB reconfiguration through the persistent owner worker.
- Independent Arena thread settings (`--a-threads` / `--b-threads`) and Release
  scaling measurements at 1/2/4/8/16 workers.
- Focused payload, collision, concurrent-writer, lifecycle, PV-legality and
  helper-shutdown tests; full validation is recorded in `docs/PERFORMANCE.md`.

## V0.10 — Advanced Alpha-Beta / Selectivity / QSearch (planned)

Evaluate each item behind regression, fixed-position benchmark, and engine-match
evidence rather than adding chess techniques by analogy:

- Continuation History and Countermove History / Countermove Heuristic.
- History Gravity + History Malus.
- An adaptive LMR table using depth, move index, PV/scout status, history, TT
  move, threat profile and cut-node context.
- Late Move Pruning, Futility Pruning, Reverse Futility Pruning, Razoring,
  ProbCut, Internal Iterative Reduction and Gomoku-adapted Singular Extension.
- Bounded Threat Extension and Mate Distance Pruning.
- Advanced Quiescence Search: better forcing filtering, stand-pat preservation,
  forcing ordering, tactical quiet/noisy classification, and delta-style pruning
  only where Gomoku semantics justify it.
- Optional tiny/gated interior VCF or VCT probes only at strongly tactical,
  narrow-branch nodes.
- Cache/data-layout tuning when profile evidence exists.

Null Move Pruning remains experimental and disabled: the chess pass assumption is
structurally unsafe in initiative-heavy Gomoku. Keep it disabled or deletable
unless Arena evidence and tactical regressions support it.

Advanced QSearch is a first-class V0.10 target. V0.8 measurements showed that
qsearch dominates some deeper positions (about 75k of 123k AB nodes in opening
D6 and about 141k of 189k in forced defense D6). The large opening database,
D4 deduplication, empirical balance suite and automatic filtering also belong to
V0.10 infrastructure/tuning work.

## V0.11 — NNUE / learned policy / SIMD (planned)

- Incrementally updatable NNUE through `Evaluator::State` / `Undo`; BoardState
  and tactical solvers remain NNUE-independent.
- Scalar reference implementation first, then quantized inference and an AVX2
  runtime path; later SIMD targets require evidence.
- Training data from deep AB/self-play/game results and VCF/VCT labels.
- A policy head or lightweight policy representation for move ordering.
- Re-tune LMR, futility and ProbCut margins after evaluator distribution changes.
- Any isolated unsafe optimization requires a later explicit policy change,
  profiling evidence and differential tests; V0.9 remains Safe Rust.

Rapfi/Figrid-style NNUE ideas may be studied conceptually, but GPL code is not
copied.

## V0.12 / research hardening if justified (planned)

- Proof-guided Alpha-Beta budget allocation.
- Learned or threat-aware decisions about when to invoke DFPN.
- Threat-aware selective reduction and quiet-threat/dependency-TSS research.
- TT/cache layout hardening, persistent search workers if thread-spawn cost is
  measured, and NUMA/server scaling on real large-CPU hardware.
- PGO/LTO/codegen tuning.

Do not create V0.12 merely to satisfy a version number.

## Later separate neural-search route

Keep a future `SearchEngine` backend for MCTS, AlphaZero, Gumbel AlphaZero,
Transformer value/policy evaluation, and PNS/DFPN-assisted neural tree search.
This route requires a training/self-play/GPU pipeline and is not part of
V0.9–V0.11 classical-engine completion. It does not replace `AlphaBetaEngine`.

## Long-term architecture

- `AlphaBetaEngine` remains the primary classical backend.
- Tactical solving is an independent engine subsystem.
- Future MCTS is another `SearchEngine` backend, not a replacement for classical search.
- Evaluator remains replaceable; board proof nodes do not update learned accumulators.
- Core owns rules and legal transitions. Search consumes Position; apps are adapters.

## Explicit V0.9 non-goals

V0.9 does not implement V0.10 selective pruning, an advanced qsearch redesign,
interior VCF/VCT, NNUE, policy networks, SIMD optimization, unsafe code, MCTS,
AlphaZero/Gumbel AlphaZero, Transformer evaluation, GPU compute, a server or
protocol layer, Renju/Standard rules, Swap/Swap2, a large opening database,
SPRT/Elo infrastructure, or a generic thread-pool/runtime framework without
measured need. Finish the small Lazy-SMP/shared-TT design first.
