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

## V0.10 — Advanced Alpha-Beta / Selectivity / QSearch (implemented)

- Signed gravity/malus main history, countermoves, and one/two-ply continuation
  history with packed deterministic ordering.
- Worker-local fixed search context and adaptive integer LMR with tactical, TT,
  killer, countermove, history, and cut-node protection.
- Guarded LMP, futility, reverse futility, razoring, IIR, exact mate-distance
  pruning, and a one-ply path-bounded strong-threat extension.
- Directional validity remains the TT firewall for every selective result; IIR
  records actual searched depth and direct heuristic returns are never cached.
- Qsearch instrumentation showed recursive forcing expansion is minor. Exact
  immediate facts, forced blocks beyond caps, stand pat, and the own Four+
  vocabulary remain unchanged; no unjustified Three expansion or delta bound was
  added.
- Null Move, ProbCut, singular extension, and interior proof probes were
  deliberately deferred because their assumptions/evidence were insufficient.

## V0.11 — Offline Solver & Proof Book (implemented)

- Added an offline solver tool with D4 canonicalization for book/database use.
- Orchestrates a best-first AND/OR frontier and bounded resources while reusing
  exact Immediate, VCF, VCT and DFPN solving.
- Produces a compact Freestyle Proof Book with proof metadata/certificates and
  an independent verifier.
- Queries the verified Proof Book at the runtime root before ordinary online solving.
- Keeps opening D4 deduplication and empirical balance metadata separate from
  proven strategy.

The semantic boundary is strict:

- Opening Suite / Opening Database: empirical experimental starts.
- Proof Book: mathematically/search-proven strategy data.
- Game Record: chronological played moves.

Empirical balance must never be described as proof.

## V0.12 — Learned Local-Pattern Value + Policy (implemented)

- Added a complete direct `LineKey` embedding table, worker-local reversible
  accumulators, quantized scalar Value, and candidate-local Policy ordering.
- Added checked model/data formats, deterministic teacher generation, game-level
  splits, training-only D4 augmentation, and PyTorch train/evaluate/export tools.
- Preserved PatternEvaluator as default/reference and all evaluator-independent
  tactical proof paths; evaluator replacement invalidates ordinary TT scores.
- Added Core and Native Undo/Redo timelines, session move timing and explicit
  result-origin presentation.
- Repaired V0.11 cached Refuted provenance, checkpoint capacity consistency,
  Proof Book verification limits, and per-pass verifier memoization.
- Safe scalar inference is the V0.12 baseline. SIMD and broad evaluator-margin
  tuning remain measured V1.0 work; no strength claim follows from smoke data.

Rapfi/Figrid-style learned-evaluation ideas may be studied conceptually, but GPL
code is not copied.

## V1.0 — Integrated Strength / Proof-Guided Search (in progress, unreleased)

Implemented research infrastructure: trajectory/lineage split manifests,
quality filtering, calibrated V1 export and equivalent folded inference;
resumable CPU training, verified proof labels, bounded smoke orchestration and
hash-bound champion gating; paired statistics/recovery and a minimal external
pbrain runner; nonlinear float reference; default-off single-worker VCF ordering
probes; existing-selectivity ablation switches; Windows/Linux CI definitions.

Release gates remain open: a stable trained model, independent fixed-time
confirmation, external strong-engine games with frozen weights and resources,
and statistical strength evidence. Current smoke samples are inconclusive.
Proof-neighborhood perturbation/relabel generation and calibrated search
profiles remain future work. E3-E6 algorithms are gated on those prerequisites;
they have not been implemented merely to fill the roadmap. No persistent SMP
pool, SIMD, or TT synchronization rewrite is justified by the current timings.

Work package 1 repairs experiment identity and recovery: Rust-described actual
player configurations, bound model export/calibration/integer evidence,
model-versus-combination promotion, match-clock/line-ending protocol compliance,
atomic immutable manifests, and relocated proof-bundle paths. These are trust
and correctness gates, not a new trained model or evidence of playing strength.
The remaining work is executed continuously as one V1.0 milestone. The current
implementation pass starts at clean `31b71a57251304778de45d495b2b99609dd26683`.
It preserves the checkout and ends uncommitted. Delivery status is tracked below;
planned work is not an implemented capability.

| Area | Current pass status |
| --- | --- |
| Compact dataset, split, streaming audit | implementation pending |
| Root teacher, exploration, calibrated labels | implementation pending |
| Width-8 integer V2, trainer and evidence | implementation pending |
| Search profiles, policy LMR, proof scheduling, singular, ProbCut | implementation pending |
| Guarded Null Move | rejected-with-evidence: immutable Position transition and side/hash/evaluator consistency conflict; see Architecture |
| Application time management and bounded experiments | implementation pending |
| Integrated validation and adversarial review | pending after implementation |
| Independent strength, hosted Windows/Linux CI, release | external-gate-pending |

Experimentally evaluate rather than automatically retain:

- proof-guided Alpha-Beta/DFPN budget scheduling;
- policy-based reductions;
- guarded Null Move with verification;
- ProbCut;
- Singular Extension / excluded-move search;
- tiny interior VCF/DFPN probes;
- statistical Arena support with paired openings, Elo/LOS/SPRT/LLR;
- final parameter tuning;
- measured TT/cache/thread-pool/NUMA/PGO hardening.

Every optional search technique remains only when correctness tests and measured
playing strength support it.

## Later V1.x/V2 research backend

Keep a separate future `SearchEngine` backend for MCTS, AlphaZero/Gumbel
AlphaZero, Transformer policy/value, and neural tree search with PNS/DFPN. It
requires a training/self-play/GPU pipeline and does not replace
`AlphaBetaEngine`.

## Long-term architecture

- `AlphaBetaEngine` remains the primary classical backend.
- Tactical solving is an independent engine subsystem.
- Future MCTS is another `SearchEngine` backend, not a replacement for classical search.
- Evaluator remains replaceable; board proof nodes do not update learned accumulators.
- Core owns rules and legal transitions. Search consumes Position; apps are adapters.

## Explicit V0.12 non-goals

V0.12 does not implement Null Move, ProbCut, singular extension, interior
VCF/VCT, policy-based reduction/pruning, SIMD optimization, unsafe code, MCTS,
AlphaZero/Gumbel AlphaZero, Transformer evaluation, GPU compute, a server or
protocol layer, Renju/Standard rules, Swap/Swap2, a large opening database,
SPRT/Elo infrastructure, or a generic thread-pool/runtime framework without
measured need.
