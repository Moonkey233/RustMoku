# RustMoku progress

- Baseline HEAD: `30886af587441a3d4d4d77fc37cba8e32c7ba1be` (`v1.0 third`); clean at start.
- Current package: W2, tactical abstraction and broad teacher/production candidates.
- Completed W0 implementation: eframe 0.36.1 X11/Wayland features; parity and
  selfplay result-type lints; typed config-relative cross-version file inputs;
  Linux fixture executable suffix/permissions; Arena reason/winner/clock audit;
  root resistance and fallback with focused regressions; development naming.
- Completed W1 implementation: shared named counter snapshots (including interior
  proofs), benchmark/GUI diagnostics, CLI model architecture/version/contract,
  explicit Native load errors preserving the active evaluator, V1/V2 benchmark load.
- Completed W2 subtask: ThreatResolver separates exact immediate facts from hints;
  distinct production/teacher/proof universes; all-legal root teacher with shared
  horizon and reusable analysis scratch; JSON v2 candidate recall; default-off
  adaptive tactical/policy root inclusion with TT score isolation and telemetry.
- Decisions: preserve trusted-bound/proof architecture; risky heuristics require
  independent switches and remain experimental without strength evidence.
- Decisions: resistance only breaks verified equal negative root scores, with
  full-window scout verification, independent switch and counters; no score bonus.
- Checks: Windows workspace all-target check and all-feature Clippy passed;
  5 Python path/receipt regressions, 4 focused root regressions, 3 teacher/universe
  regressions passed. Debug CLI smoke compared all 224 legal replies at depth 1;
  two one-move Arena legs passed real clock/record verification. No final gate yet.
- Risks: hosted Linux CI unverified; schema-1 clocks omit startup durations,
  so initial clock receipts allow bounded startup cost, later moves audit exactly
  within 1 ms truncation. No performance or playing-strength claim.
- Next: finish W2 defense/dependency and bounded qsearch continuation work, then
  W3 LMR/improving/IID/experimental search. W3-W11 remain unimplemented in this
  upgrade. Run the single consolidated final gate only after main implementation.
