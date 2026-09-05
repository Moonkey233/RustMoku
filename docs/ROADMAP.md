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

## V0.8: Search limits / cancellation / SearchInfo / Arena

- Explicit limits, cancellation, and incremental search information.
- Reproducible engine matches and strength measurement.
- Keep time management and application scheduling outside board semantics.

## V0.9: Multi-core Lazy SMP / shared TT

- Measured parallel classical search with explicit ownership and synchronization.
- Preserve independent worker state and documented shared-table semantics.

## Later

- Advanced selective tuning, with regression, benchmark, and match evidence.
- NNUE and custom-evaluator API milestone: address the public PatternState debt.
  PatternUndo is private; the existing Evaluator abstraction stays intact in V0.7.
  Decide which pattern features an external evaluator should access and whether
  engine tactical state belongs in its public evaluation context.
- Additional rules and opening protocols, modeled as distinct concepts.
- Analysis tools and server deployment.
- MCTS, policy/value evaluation, and reproducible self-play.

## Long-term architecture

- AlphaBetaEngine remains the primary classical backend.
- Tactical solving is an independent, shared engine subsystem.
- Future MCTS is another SearchEngine backend, not a replacement for classical search.
- Evaluator remains replaceable; board proof nodes do not update learned accumulators.
- Core owns rules and legal transitions. Search consumes Position; apps are adapters.

V0.7 does not implement V0.8 deadlines, cancellation, SearchInfo, Arena,
multicore search, NNUE, MCTS, new rules/openings, unsafe Rust, or SIMD.
