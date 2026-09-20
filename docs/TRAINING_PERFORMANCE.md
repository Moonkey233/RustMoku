# MixLite V3 production hot path

The production trainer caches **inputs only**. Model architecture, integer export,
targets, sample weights, mining cadence, optimizer, split and sampler identities
are unchanged. `mixlite.features` and `reference_loss` remain independent slow
references. Ranking loss still uses the original per-record implementation.

## Feature cache

The immutable cache contains four `uint16` line keys and one `uint8` relative
occupancy per cell: **2025 bytes per training record**. It uses a read-only NumPy
mmap, with bounded construction batches of 256 records. Its manifest binds the
dataset fingerprint, complete split manifest, ordered training-index digest and
feature schema. Payload length and SHA256 are checked on reuse. A mismatch fails
clearly; select a fresh directory to rebuild. Publication renames a completed
temporary directory; interrupted construction cannot publish a partial cache.

The default cache is `<checkpoint directory>/feature-cache`, with a 4 GiB
payload limit. `--cache-dir` and `--cache-max-bytes` configure this performance
resource separately from semantic checkpoint configuration. The cap is a disk
payload cap, not a hard process RSS or OS page-cache bound. There is no object or
Torch tensor per cached record. Labels/comparisons still come from the original
dataset on every read.

D4 augmentation remains `(seed + epoch + dataset_index) % 8`. Each D4 operation
maps a direction to plus/minus another direction. Reversal-min line keys remove
the sign; inverse cell **and direction-column** permutations reproduce the slow
reference tensor exactly. Retaining direction order also preserves embedding
sum order. Policy targets use the unchanged `transform_position`/`transform_index`.

## Batched loss and diagnostics

Soft training batches WDL/value/outcome losses and legal-masked hard policy CE.
Optional comparisons retain their own observed move sets, ordered probabilities,
and padded observation masks. An absent comparison contributes zero, including
conflicted sidecar positions. Exact/hard/sample weighting and mining cadence are
unchanged. Batched floating reductions are numerically equivalent, not promised
bit-identical over an entire training trajectory. Tests compare loss, gradients
and one Adam step (parameter tolerance `atol=2e-6, rtol=1e-7`).

`--timing-output <file.json>` enables synchronized stage timings, excluding the
first five optimizer steps. It reports metadata fetch, feature/cache preparation,
H2D, forward, loss, mining sync, backward/optimizer, records/sec and steps/sec.
Stage synchronization is diagnostic overhead and is disabled in normal runs.
Cache creation and initial split validation are outside steady-step timing;
`training_seconds` includes final validation/checkpoint publication. No pinned
staging or asynchronous transfers are introduced without evidence of H2D cost.

## Reproduce a bounded benchmark (PowerShell)

These commands copy the source checkpoint into a separate directory, preserve
its SHA256, and run each path from that same snapshot. Existing benchmark outputs
are rejected; use a new directory for a new experiment. No dataset is regenerated.

```powershell
.\.venv-train\Scripts\python.exe -X utf8 training\benchmark_mixlite.py --dataset datasets\v3-serious-gen0\dataset.json --resume runs\v3-serious-gen0\checkpoint.pt --output runs\v3-hotpath-benchmark --mode reference --steps 100 --device cuda
.\.venv-train\Scripts\python.exe -X utf8 training\benchmark_mixlite.py --dataset datasets\v3-serious-gen0\dataset.json --resume runs\v3-serious-gen0\checkpoint.pt --output runs\v3-hotpath-benchmark --mode cache-only --steps 10 --device cuda
.\.venv-train\Scripts\python.exe -X utf8 training\benchmark_mixlite.py --dataset datasets\v3-serious-gen0\dataset.json --resume runs\v3-serious-gen0\checkpoint.pt --output runs\v3-hotpath-benchmark --mode optimized --steps 100 --device cuda
```

## Resume the paused Serious run

Resume from the **original** checkpoint, not a benchmark output. The existing
checkpoint production configuration is checked verbatim. For the current seed-17
Serious run (batch 256, mining every 16 steps):

```powershell
.\.venv-train\Scripts\python.exe -X utf8 training\mixlite_production.py --dataset datasets\v3-serious-gen0\dataset.json --output runs\v3-serious-gen0\checkpoint.pt --resume runs\v3-serious-gen0\checkpoint.pt --device cuda --batch-size 256 --seed 17 --steps 50000 --epochs 20 --learning-rate 0.001 --policy-target soft --exact-weight 4 --hard-weight 2 --outcome-weight 0.25 --mining-every 16 --checkpoint-every 100 --cache-dir runs\v3-hotpath-benchmark\feature-cache
```

`--steps` is an additional-step ceiling; `--epochs 20` retains the original epoch
ceiling. This direct trainer command resumes training only; it does not perform
the production wrapper's subsequent export/calibration/evidence stages.
