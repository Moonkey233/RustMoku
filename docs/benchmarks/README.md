# Benchmark evidence

All data is observational, captured on 2026-09-05 on the host described in
../PERFORMANCE.md. V0.1/V0.2 historical tables are preserved there.

- v02-quick.csv / v02-deep.csv: unchanged 08232e4 engine, repeated cold-run harness.
- v02-search-bench.rs: exact source snapshot of that harness (not a workspace target).
- v03-quick.csv / v03-deep.csv: final version 0.3.0 default PatternEvaluator.
- v03-classical-quick.csv / v03-classical-deep.csv: reference evaluator control;
  all ten semantic results match the V0.2 baseline.
- v03-tt-sizes.csv: final depth-8 opening, four TT capacities, median of three.
- v03-hotpath.csv: final optional microbenchmarks, median of five.
- profile-before.csv / profile-before-ordering.csv: WPR/xperf CPU samples filtered
  to the benchmark process. sampled_microseconds sum over the process; the xperf
  percent column is relative to the full trace/all CPUs, not the process.

To reproduce the V0.2 remeasurement in an isolated checkout, from the current
repository root (choose an unused sibling directory):

```powershell
git worktree add --detach ../RustMoku-v02 08232e4
Copy-Item docs/benchmarks/v02-search-bench.rs ../RustMoku-v02/crates/rustmoku-engine/examples/search_bench.rs
cargo run --release --manifest-path ../RustMoku-v02/Cargo.toml -p rustmoku-engine --example search_bench
cargo run --release --manifest-path ../RustMoku-v02/Cargo.toml -p rustmoku-engine --example search_bench -- --suite deep --repeats 3
```

For V0.3 commands and interpretation, see ../PERFORMANCE.md. The profiler ETL
files (system-wide captures) remain under ignored target/v03-evidence; only the
process-filtered summaries are included in the repository.
