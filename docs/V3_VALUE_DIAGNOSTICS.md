# V3 value-path investigation (no architecture change)

## Separation experiments

Arena accepts per-player `--a-v3-mode` / `--b-v3-mode`:

| Mode | Value | Policy | Score contract |
|---|---|---|---|
| normal (default) | V3 | V3 | RationalV2 |
| value-only | V3 | None | RationalV2 |
| policy-only | Pattern | V3 | Pattern |

The adapter exists only in Arena. All modes maintain V3 incremental state;
policy-only therefore remains an ordering experiment, not a speed optimization.
Normal preserves the existing model fingerprint. Diagnostic modes hash the mode,
V3 fingerprint, and (for policy-only) Pattern fingerprint into a separate
evaluator identity. `--describe` exposes `v3_mode`, `evaluator_fingerprint`, model
SHA256 and effective score contract/profile. Explicit modes require a V3 model;
Pattern, external, V1/V2, mismatched profiles/calibration/books fail closed.
Native, engine evaluator behavior and model bytes are unchanged.

PowerShell, from the workspace root (12 pairs, reversed colors, 50k work/move;
no additional time limit):

```powershell
cargo build --release -p rustmoku-arena
$model = 'runs\v3-serious-gen0\model-12513465bb24e3e9.rmlp'
.\target\release\rustmoku-arena.exe --pairs 12 --depth 64 --nodes 50000 --a-model $model --a-v3-mode normal --b-evaluator pattern > runs\v3-normal.csv 2> runs\v3-normal.log
.\target\release\rustmoku-arena.exe --pairs 12 --depth 64 --nodes 50000 --a-model $model --a-v3-mode value-only --b-evaluator pattern > runs\v3-value-only.csv 2> runs\v3-value-only.log
.\target\release\rustmoku-arena.exe --pairs 12 --depth 64 --nodes 50000 --a-model $model --a-v3-mode policy-only --b-evaluator pattern > runs\v3-policy-only.csv 2> runs\v3-policy-only.log
```

Append `--a-disable all --b-disable all` consistently to all three commands for a
separate no-selectivity experiment. The normal/value-only policy removal changes
ordering and hence reached horizons; it does not measure value error in isolation
from search. Twelve pairs are diagnostic evidence, not a precise Elo estimate.

## Read-only diagnostics

```powershell
.\.venv-train\Scripts\python.exe -X utf8 training\diagnose_value.py --dataset datasets\v3-serious-gen0\dataset.json --checkpoint runs\v3-serious-gen0\checkpoint.pt --split train opening_heldout --samples 2048 --seed 17 --device cuda --output runs\v3-final-value-diagnostics.json
.\.venv-train\Scripts\python.exe -X utf8 training\diagnose_value.py --dataset datasets\v3-serious-gen0\dataset.json --checkpoint runs\v3-hotpath-benchmark\input.pt --split train opening_heldout --samples 2048 --seed 17 --device cuda --output runs\v3-step3800-value-diagnostics.json
```

The sample is seeded uniform sampling without replacement from each **validated
qualified partition**, sorted for disk locality. Indices are hash-bound in the
report. Across checkpoints, identical split/seed/count produce identical sample
positions. Train and heldout remain distinct positions and lineages; no heldout
example participates in training. Evaluation uses canonical symmetry zero;
training augmentation still follows the production schedule.

Reports include value MAE/correlation/sign accuracy (`abs(target)>=0.05`), moments
and quantiles, WDL probabilities and raw evidence, source/exact strata, policy
recall, five separate loss components, parameter clamp/sign/displacement
statistics, and per-component/per-family gradients on the first fixed batch.
`value_total` includes teacher CE, q squared error and configured outcome loss;
`policy_total` includes hard and optional soft supervision. Gradient norm ratios
and cosines are local diagnostics, not evidence of accumulated Adam updates.

Raw component means divide by **all sampled records**, with absent supervision
contributing zero. Weighted contributions are reported separately for ordinary
and mining batches, plus the configured cadence expectation; this is not a
reconstruction of the historical training schedule. For value-only checkpoints,
the same reference-production diagnostic additionally reports actual value-only
objective contributions (policy and outcome zero). Constant-target/prediction
correlations and zero-norm gradient cosines are null, not fabricated numbers.

## Controlled value-only training

```powershell
.\.venv-train\Scripts\python.exe -X utf8 training\train_value_only.py --dataset datasets\v3-serious-gen0\dataset.json --reference runs\v3-serious-gen0\checkpoint.pt --baseline runs\v3-hotpath-benchmark\input.pt --cache-dir runs\v3-serious-gen0\feature-cache --output runs\v3-value-diagnostic --device cuda --steps 2000 5000 --samples 2048 --sample-seed 17
```

Use a fresh output directory; existing outputs are rejected unless `--resume`
is supplied with the identical configuration/reference. A `latest.pt` diagnostic
checkpoint is saved every 100 steps and at each milestone; complete hash-bound
reports are reused. Repeat the same command with `--resume` after interruption.
This one invocation
validates the frozen corpus/split once, evaluates both reference checkpoints on
the fixed train/heldout samples, resets to fresh seed-17 initialization and fresh
Adam, then writes `step-2000.pt`, `step-5000.pt` and matching JSON reports. It
retains the reference's scale, batch size (256), LR (.001), block shuffle and D4
schedule. The only optimized objective is teacher WDL CE + q squared error:
no policy or outcome loss. Exact/sample weights and hard-mining cadence are
preserved; detached current policy predictions are used only for those mining
weights. The policy head/context parameters receive no gradient.

The script has a hard 5000-step diagnostic ceiling and writes no production
checkpoint. Its checkpoint has `diagnostic`/`source_production` metadata, not a
`production` resume declaration, so the production trainer rejects accidental
resume. It is not a model promotion/export pipeline. Cache bytes are read through
the existing identity checks, and the Serious shards are never written.

## Current value design: what can and cannot be concluded

The unchanged scalar/QAT network maps local four-line embeddings and center
occupancy into 32 clamped features per cell. D4-invariant Chebyshev bands have
9/40/72/104 cells. Global plus four band averages form 160 pooled features,
followed by an 8-dimensional nonnegative context and three 8-weight evidence
heads. Evidence is `relu(trunc(head dot context / 16)) + 1`; WDL is normalized
evidence, and value is `P(win)-P(loss)`.

Pooling loses within-band angular arrangement and relative positions between
distant motifs beyond what the local feature fields encode. Two positions with
the same pooled 160-vector necessarily have identical context and value. This is
a statement about the representation: no legal tactical collision fixture has
yet been exhibited, and observed fitting failure alone does not establish that
this information loss is the dominant cause. The contextual policy additionally
sees the candidate's local 32-vector.

Both pooling (`trunc(sum/count)`) and context construction (`trunc(dot/256)`,
then clamp to 0..255) discard fractional changes. A sparse local change can
therefore disappear in the pooled integer representation even before spatial
arrangement is considered. This is a structural observation, not a measured
collision rate or proof that sparse-threat dilution explains this corpus.

The WDL head initializes uniformly in `[1,256]`, not near zero; embedding/center/
mixing use normal std 12, bias starts at 512. Positive heads over nonnegative
context tend to express ratios of shared evidence rather than near-one-hot WDL.
For nonnegative integer head columns, the **pre-truncation** q values lie in the
convex hull of `(win-loss)/(win+draw+loss)` column ratios and the zero prior.
This is not advertised as an exact post-integer-arithmetic bound.

Direct parameter inspection before corpus diagnostics found:

- Step 3800: 23 positive / 1 zero / 0 negative integer WDL weights; displacement
  from seed-17 initialization about -2.02 to +1.95; pre-truncation column q range
  approximately [-0.3610, +0.3478].
- Step 34540: 23 positive / 0 zero / 1 negative integer weights; displacement
  about -19.76 to +17.13. Neither checkpoint hits int16 head clamp limits.

There is also a conservative **post-truncation evidence-ratio** bound when all
integer head weights are nonnegative and every column sum is positive. With
`T_min = min(column sums)/16` and column ratio extrema `r_min/r_max`, nonzero
integer context gives
`(r_min*T_min-1)/(T_min+1) <= q <= (r_max*T_min+1)/(T_min+1)`; zero context gives
zero. This follows because each `floor(raw)+1` differs from its nonnegative raw
head by an amount in `(0,1]`. For seed-17 initialization this bounds q by roughly
[-0.400063, +0.385910], and step 3800 by [-0.398208, +0.385771], regardless of
the upstream context mapping. It applies before runtime Q15 probability
rounding. The helper refuses signed-head cases (including final step 34540),
rather than extending the proof without justification. A focused test checks
zero, every single-axis integer activation and seeded mixed contexts.

An Adam learning rate of .001 is not a strict per-step displacement bound.
These measured movements show why thousands of QAT steps need not substantially
reparameterize a head initialized at order 100. They motivate the controlled
experiment, not a width increase. A successful value-only fit would implicate
multi-task optimization; failure at 5k steps still cannot distinguish insufficient
representation from initialization/optimization difficulty. Train/heldout
metrics and conditional gradient evidence must be read together before proposing
V4 or claiming that policy gradients dominate.

## Measured controlled experiment (2026-09-21)

Completed CUDA batch-256 training from fresh seed 17 through 5000 steps, with
2000/5000 checkpoints. Training-loop elapsed time, including milestone reporting,
was 754.2 seconds; corpus admission and baseline diagnostics are additional.
Artifacts are in `runs/v3-value-diagnostic/`: `reference.json`,
`baseline-0-step-3800.json`, `step-2000.json`, and `step-5000.json`.
All four reports use the same 2048 positions per partition. Train sample SHA256
is `32a328d2e80e3fb8d31ed518ab63ebcff993a682384ee85c9e1095f110d24e72`;
heldout is `429be6f3f756a7d12e7557a03fbb1c3589fd0cbbea310ca5c36f3e7bb6bab230`.
These samples need not match earlier externally reported diagnostics.

| Checkpoint | Split | MAE | Correlation | Sign accuracy | Prediction std |
|---|---|---:|---:|---:|---:|
| Multi-task 3800 | train | .59235 | .13000 | .51200 | .03108 |
| Multi-task 3800 | heldout | .58837 | .13743 | .51319 | .02960 |
| Multi-task 34540 | train | .57816 | .22773 | .54308 | .09107 |
| Multi-task 34540 | heldout | .57592 | .21496 | .54958 | .09535 |
| Value-only 2000 | train | .58217 | .18528 | .52999 | .07935 |
| Value-only 2000 | heldout | .57884 | .21142 | .53745 | .06995 |
| Value-only 5000 | train | .57017 | .27334 | .55780 | .10563 |
| Value-only 5000 | heldout | .56845 | .25425 | .54272 | .11196 |

Target std is .67713 train / .67529 heldout. Value-only 5000 heldout predictions
span only [-.37273, .19149], despite targets spanning [-1, 1]. Removing policy
and outcome objectives modestly improves MAE/correlation, but does not resolve
compression, and heldout sign accuracy does not beat the final multi-task model.
This is one seed and a finite learning budget, not a capacity impossibility result.

On the final multi-task checkpoint's first fixed train batch, ordinary-batch
policy/value raw gradient norm ratios are 291.6 for embedding and 54.6 for center.
Mixing/bias ratios are .656/.466, with cosine similarities -.396/-.583.
Heldout gives embedding/center ratios 255.2/42.6 and mixing/bias cosines
-.495/-.508. Thus policy dominates these local embedding/center gradient norms;
context gradients instead show comparable magnitudes and opposing directions.
The batch is the first 256 entries of the sorted sample, not an aggregate over
all training batches, and these are pre-Adam gradients.

The value-only head moved only -2.463..+2.362 from initialization at 5000 steps,
with 23 positive and one negative integer weight. The nonnegative-head bound
therefore no longer applies there. At 2000, all 24 remained positive and the
conservative bound was [-.40091, .38577]. Head parameterization/optimization is
a concrete concern alongside shared-gradient conflict; widening the network is
not justified by this experiment alone. No legal pooled-feature collision has
been demonstrated, so spatial capacity remains an unmeasured hypothesis.

Both diagnostic checkpoints preserve policy-head/context tensors exactly at
initialization and omit production-resume metadata. Checkpoint/report hashes
match. The original Serious checkpoint still hashes to
`12513465bb24e3e987bc102ffcd21c8cf657d64ae16aa21358d3ab24145705db`.
No original checkpoint or dataset shard was modified. No decomposition Arena
games were run; only the three modes' real-model `--describe` preflights ran.

Validation: focused Arena lifecycle/contracts test, Arena clippy and release
build; Python component/reference-loss, deterministic reporting, head-range,
and interrupted-resume tests. The tiny resumed run matched continuous CPU
parameters exactly. No full workspace suite or new strength campaign was run.
