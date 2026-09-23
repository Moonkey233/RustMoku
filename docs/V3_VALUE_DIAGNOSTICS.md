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

## Controlled head initialization / optimization matrix

`training/experiment_wdl_head.py` runs fresh A/B/C/D arms with the existing
value-only objective, one shared read-only corpus validation and the same cache.
The small-positive head affine-maps the production uniform draws from [1,256]
to [1,8]; this preserves every other initial tensor and consumes no extra RNG.
Only C/D use a separate Adam group for the head at .016; all other parameters
remain at .001. No signed initialization, evidence/QAT change, architecture
change or production-default change is involved.

```powershell
.\.venv-train\Scripts\python.exe -u -X utf8 training\experiment_wdl_head.py --dataset datasets\v3-serious-gen0\dataset.json --reference runs\v3-serious-gen0\checkpoint.pt --cache-dir runs\v3-serious-gen0\feature-cache --output runs\v3-head-matrix --device cuda
```

Use `--resume` only for the same existing output directory and inputs; do not
launch a concurrent copy. A/B/C/D subdirectories contain independent manifests,
latest checkpoints and 2k/5k reports. The matrix inventories the original
descriptor, all shards, comparison sidecar and reference checkpoint before and
afterward. The completion receipt is written only if their hashes still match.
Each arm also binds the split/configuration and hashes the actual sequence of
dataset indices and D4 transforms, allowing direct cross-arm comparison.
The mining rule/cadence is unchanged; actual hard-example weights can differ
because the arms make different predictions. They are not artificially frozen.

Reports add pre-ReLU evidence (after the declared integer truncation), each
class's nonpositive fraction, probability concentration above .8/.9/.95,
distinct integer head weights, displacement from that arm's initialization,
and `teacher_value_only` gradient norms separately from counterfactual
production losses. Nonfinite loss/gradients fail that arm explicitly; the
matrix continues the other arms without reducing its learning rate.
`final_batch_objective` is the actual last minibatch loss, not a full-corpus
objective or a model-selection criterion. Per-arm time is captured when saving
the milestone, includes earlier reports/writes, and excludes that milestone's
subsequent report. Matrix wall time includes all reports, shared admission and
final input-hash verification.

## Head matrix results (2026-09-22)

All four arms completed 2000 and 5000 steps. Each uses 442,098 qualified train records after the existing exact-label filtering. This equals the preceding diagnostic run (the earlier 442,104 summary was not its actual filtered count). No corpus, split or filtering semantics changed.

A = production init / 1x; B = small-positive init / 1x; C = production init / 16x; D = small-positive init / 16x. Each row evaluates the same 2048 train and 2048 opening-heldout records. Sign accuracy uses abs(target) >= .05 and counts zero predictions as incorrect for a nonzero target.

| Arm | Steps | Train MAE / r | Heldout MAE / r | Heldout prediction std | Heldout sign accuracy |
|---|---:|---:|---:|---:|---:|
| A | 2000 | 0.5822 / 0.1853 | 0.5788 / 0.2114 | 0.0700 | 53.74% |
| A | 5000 | 0.5702 / 0.2733 | 0.5685 / 0.2542 | 0.1120 | 54.27% |
| B | 2000 | 0.5826 / 0.1737 | 0.5755 / 0.1942 | 0.0945 | 39.93% |
| B | 5000 | 0.5446 / 0.3588 | 0.5424 / 0.3718 | 0.1847 | 36.18% |
| C | 2000 | 0.5821 / 0.1894 | 0.5780 / 0.2211 | 0.0738 | 53.85% |
| C | 5000 | 0.5601 / 0.2987 | 0.5533 / 0.3272 | 0.1563 | 58.81% |
| D | 2000 | 0.5440 / 0.3264 | 0.5413 / 0.3240 | 0.2636 | 42.67% |
| D | 5000 | 0.4974 / 0.4269 | 0.5097 / 0.3947 | 0.3698 | 44.36% |

Target mean/std: train .01352/.67713; heldout .02858/.67529. At 5000, heldout prediction mean is A .08460, B .07092, C .09987, D .10329. Full moments, quantiles, per-source/exact strata, WDL/evidence distributions, pre-ReLU fractions, loss components and gradient families are in each `runs/v3-head-matrix/{A,B,C,D}/step-{2000,5000}.json`.

### Interpretation and limitations

- Both interventions affect the value path. At 5000, B improves heldout correlation from .2542 to .3718 and C to .3272. D reaches .3947 and the broadest prediction distribution (.3698 std), with the lowest overall heldout MAE (.5097). Initialization/head learning-rate difficulty is therefore not ruled out; the all-arms-stay-compressed outcome did not occur.
- D is not an unqualified winner. B/D heldout sign accuracy drops to .3618/.4436, compared with A .5427 and C .5881. Both B/D have prediction 25th percentile and median exactly zero. D pre-ReLU nonpositive percentages are win 50.93%, draw 65.63%, loss 69.29%; B gives 41.55%/54.79%/48.78%. The evidence floor and quantized ties remain an important observed tradeoff, despite the strictly positive initialization.
- Improvements are concentrated in exact/tactical labels. A to D heldout exact MAE improves .9233 -> .6239 and correlation .4721 -> .6798. For non-exact AB labels, MAE barely changes .4863 -> .4833 and correlation .1804 -> .2129; B/C non-exact correlations are .2120/.2224. Broader output range alone has not solved ordinary position-value learning.
- No nonfinite loss/gradient event occurred, and all saved model tensors are finite. No parameter hits its quantization clamp endpoint. At D-5000, 5.81% of heldout positions have some WDL probability > .8; none exceeds .9 or .95. Other 5k arms have none above .8. This rules out observed extreme probability saturation in these samples, not all possible activation saturation or future instability.
- At 5000, head displacement from its own initialization is A -2.463..+2.362, B -3.107..+2.789, C -35.774..+37.297, D -16.516..+18.342. Integer negative/zero/positive counts are A 1/0/23, B 2/2/20, C 2/0/22, D 8/1/15; distinct integer counts are 23/9/23/19. Smaller initialization makes similarly sized updates significant relative to initial weights.
- On the first fixed heldout batch, actual teacher-only ordinary head gradient norms are A .00125, B .02188, C .00132, D .01904. Full context/trunk norms are recorded. These are local pre-Adam measurements, not accumulated optimizer updates.
- This is one seed and a finite 5k budget. It supports initialization/optimization as contributing factors, but neither proves representational sufficiency nor justifies immediate V4 redesign. Production defaults stay unchanged. No diagnostic checkpoint was exported/promoted or played in Arena.

### Integrity, runtime and validation

The matrix completed in 3904.9 seconds (65.1 minutes), including shared admission and final hash verification. Per-arm recorded training/milestone time was A 782.2 s, B 788.3 s, C 788.3 s, D 786.5 s. Final minibatch objectives were 2.6420, 2.5117, 2.5670, 2.3506 respectively; these are not selection criteria.

All eight reports have matching train/heldout sample hashes listed above. Record/D4 sequence hashes match across all arms at both milestones:

- 2000: `ec972ca3392af50cbfda00a301b45ae792cc3ca3d098f9585326781b23fdd3bd`
- 5000: `5a837dcd486efccb8cd1d69d7a3c37a0c9ac236177fdb101f6164d2e7236aa60`

Dataset fingerprint remains `58d291d2965853827710a76d59ca02a6913ea7f8d66c635ed6facdbb1021d639`; split-manifest hash is `057274931c2392e09cdb9c21a7c5bf518eaf9708b2b35d021d5986b842a63aeb`. Original checkpoint SHA remains `12513465bb24e3e987bc102ffcd21c8cf657d64ae16aa21358d3ab24145705db`. `matrix.json` inventories all original shard/sidecar/descriptor/reference hashes; `completion.json` confirms identical input bytes, inventory SHA `0f5118b81c2373174f8b3685a22cf8707d9d5c78c4a07f45a3be4ef0a8b71867`.

Arm configuration hashes:
- A: `f1cd5c67f6478bd060cf187d13a9da2af99de1921e8e82625fead4d73adf0abf`
- B: `33a3b8df749db64d666be28540bead616cf378a6eda77bd302f00c6df71a7e1e`
- C: `1d8b02e896711a1ee76ca161ef9b9024955a7f0b30cbe07823a5dc3dbf2663b5`
- D: `59353dff001022724ba98ddcf5a0ff92b09edc2809eaeeafe31fbf755feb8395`

Both A checkpoints reproduce every tensor of the preceding control experiment exactly. All diagnostic policy head/context tensors remain at initialization; optimizer groups have the requested rates, and diagnostic checkpoints contain no production-resume declaration. Report/checkpoint hashes were verified for all eight artifacts.

Validation: all five targeted value-diagnostic tests passed, including initial trunk equality, head-only LR changes, instrumentation/reference-loss checks, and a D-arm interrupted CPU resume matching continuous parameters exactly. The arm-relative displacement assertion was additionally run after being added. Python compilation and diff whitespace checks passed. No full Rust/Python workspace suite or Arena campaign was run.

## Source-2 memorization and float-relaxation probe (2026-09-23)

`training/probe_source2_memorization.py` is a diagnostic-only, resumable two-arm experiment. It selected 4096 distinct non-exact source-2 positions from the frozen qualified train partition and a disjoint 2048-position opening-heldout source-2 sample. A disk-backed SQLite selection excluded keys with conflicting raw targets; `runs/v3-source2-memorization/samples.json` contains every exact dataset index and the completed-depth distributions. The train depth counts are 1:420, 2:443, 3:1295, 4:1291, 5:414, 6:233; heldout counts are 1:195, 2:272, 3:634, 4:624, 5:191, 6:132. The source-2 eligible occurrence counts scanned were 359953 train and 40216 opening-heldout.

```powershell
.\.venv-train\Scripts\python.exe -X utf8 training\probe_source2_memorization.py --dataset datasets\v3-serious-gen0\dataset.json --reference runs\v3-serious-gen0\checkpoint.pt --cache runs\v3-serious-gen0\feature-cache --output runs\v3-source2-memorization --device cuda --resume
```

Omit `--resume` only for a new output directory. An interrupted arm resumes from its own latest checkpoint; completed milestones are not rerun.

Both arms start with bit-identical V3 learned tensors, small-positive WDL-head initialization, Adam LR .001 for non-head tensors and .016 for the head. They train the same teacher WDL CE plus normalized-q squared-error objective, batch 256, seed 17, identical repeated sample order and D4 transforms. Policy and outcome losses are absent. Online hard mining is off in **both** arms: its prediction-dependent weights would otherwise be a second arm-dependent intervention. All selected records are non-exact; ordinary sample weights remain in the objective. FloatRelaxed uses the same width-32 local features, radial/global 160-feature pooling, 8-context dimensions, WDL evidence topology and ReLU/clamp, removing only parameter rounding and integer truncation from the forward path. This is a memorization comparison, not an exportable model or a production training change.

| Arm | Steps | Train MAE / r / pred. std | Heldout MAE / r / pred. std | Train sign | Heldout sign |
|---|---:|---:|---:|---:|---:|
| QAT | 2000 | .4895 / .2773 / .1382 | .5169 / .1472 / .1650 | 50.31% | 47.35% |
| FloatRelaxed | 2000 | .4889 / .2852 / .1331 | .5187 / .1380 / .1482 | 60.82% | 54.19% |
| QAT | 5000 | .4220 / .5152 / .2707 | .4898 / .2694 / .2716 | 65.26% | 52.65% |
| FloatRelaxed | 5000 | .4063 / .5665 / .2725 | .4915 / .2672 / .2840 | 73.83% | 61.41% |
| QAT | 10000 | .3319 / .7147 / .3726 | .4696 / .3438 / .3592 | 76.50% | 57.83% |
| FloatRelaxed | 10000 | .3082 / .7654 / .3763 | .4702 / .3456 / .3682 | 85.63% | 64.22% |

The train target has mean -.0107/std .5950; heldout mean -.0004/std .6065. At 10k, FloatRelaxed improves train correlation by .0508 and MAE by .0237 over QAT, but its heldout MAE and correlation are effectively unchanged. Both numerical paths partially memorize the 4096 keys; neither strongly fits them after this bounded budget. The gap implicates discretization as a **contributing** optimization constraint, not the sole failure. FloatRelaxed still has substantial train error and compressed predictions, consistent with a representation or remaining optimization bottleneck. This experiment does not isolate topology from all optimizer/budget effects, prove a feature collision, justify V4 by itself, or establish game strength. No production default/model/checkpoint was changed.

Each milestone JSON in `runs/v3-source2-memorization/{qat,float-relaxed}/step-{2000,5000,10000}.json` contains per-depth metrics, target/prediction quantiles, WDL and raw-evidence distributions, nonpositive pre-ReLU fractions, probability concentration, parameter and fixed-batch gradient statistics, time, objective, and nonfinite count. At 10k, the fixed-batch WDL-head gradient norms were QAT .01113 and FloatRelaxed .01012; neither head reached an integer clamp endpoint. Per-arm recorded training time was 114.4 and 88.7 seconds respectively (excluding the shared frozen-data admission). Both report zero nonfinite events. These are local diagnostics rather than accumulated optimization or wall-time comparisons of the full pipeline.

Input and schedule identity:

- dataset descriptor SHA256 `0d634b7b9592c2b38835508e66ad55f84e55021bfbb4f9238793b13fb367ec1f`; reference checkpoint SHA256 `12513465bb24e3e987bc102ffcd21c8cf657d64ae16aa21358d3ab24145705db`; both unchanged after the run, and all 625 raw shards plus the comparison sidecar still match their frozen descriptor hashes;
- frozen split SHA256 `057274931c2392e09cdb9c21a7c5bf518eaf9708b2b35d021d5986b842a63aeb`, score scale 78740;
- selected sample SHA256 `b5fc66fcbad0d1c070cc357bb9cd60e0fe1da91864be66aee0a5e7ef133d39fb`; train index SHA256 `29d3d4407989c5bc4ea8e621f465fc385b1826d9c86c5d1068fbb849f75cf84e`; heldout index SHA256 `92db21bbd1457ec2bca30a096a64d5a778973ff176ca105297fbb97739e26a43`;
- arm configuration SHA256: QAT `8ffb50de424bb982c09205aa33a16c896ccdaf64e2dbca390d284bc0331502b3`; FloatRelaxed `f0271984562dce445fb8441c8ca4246a151237fafae9f1abbdc9a1d604655a69`;
- identical record/D4 sequence SHA256 at 2k `e7cd3deb706a5695f1bcee381c3512d24377d987355b7b6c1b7e4f37c4d78e37`, 5k `21775253e85cb58c58da4f4c8d8b367514219cbf6d69093f94a22634aef9c4bc`, 10k `7d17d91b3eecc112b8c00abf072c15bb8dca77ca8ac9bb77c2b76eba52576537`.

The diagnostic checkpoint format is deliberately unsupported by production resume/export loading. Interrupted execution resumed from QAT step 8000 without repeating earlier steps; a focused CPU fixture matched uninterrupted and resumed model tensors exactly for each arm. No Arena, search, regeneration, V4 design or full workspace gate was run.
