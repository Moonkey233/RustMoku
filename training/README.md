# RustMoku learned-model training

The V0.12 training model uses the engine's complete 65,536-entry directional
`LineKey` space (the nine-cell window omits its always-known center), a shared
16-dimensional embedding, a global Value accumulator, and a local candidate
Policy head. PyTorch is required only here; production inference is Safe Rust
with deterministic quantized integer arithmetic.

Generate deterministic labels with a fixed node budget and one teacher thread:

```powershell
cargo run --release -p rustmoku-data -- selfplay --games 100 --seed 7 --workers 4 --depth 8 --nodes 50000 --output data/train.rmd
cargo run --release -p rustmoku-data -- inspect --dataset data/train.rmd
```

Existing `RustMoku 1` game records can be labelled with `rustmoku-data record`.
The binary dataset stores the collision-free canonical position, the explicit
original-to-canonical D4 symmetry, game ID and ply, teacher score, canonical
one-hot Policy target, exact-label flag, and result source. Generated datasets
and model artifacts are intentionally not checked into Git.

Dataset version 1 is fixed to Freestyle and little-endian encoding. Its 16-byte
header is magic `RMDATA01`, u16 version, zero u16 flags, and u32 record count.
Each fixed 76-byte record is u64 game ID, u16 ply, u8 D4 tag, u8 canonical Policy
move (`255` means absent), i32 teacher Value, u8 result-source tag, u8 exact flag,
and the 58-byte collision-free canonical position key. Both Rust and Python
check the count, exact file length, tags, cells, padding, and side to move before
using records.

Train, evaluate, and export:

```powershell
python -X utf8 training/train.py --dataset data/train.rmd --output models/run.pt --epochs 10 --seed 7
python -X utf8 training/evaluate.py --dataset data/train.rmd --checkpoint models/run.pt --split validation --seed 7
python -X utf8 training/export.py --checkpoint models/run.pt --dataset data/train.rmd --output models/run.rmlp
python -X utf8 training/inspect_model.py --model models/run.rmlp
```

Splitting groups complete canonical trajectories and declared lineage before
sampling or augmentation. Local game IDs alone are insufficient. D4
augmentation is training-only; validation and test retain a deterministic
canonical view. The float Value target is normalized to `[-1, 1]`; export
calibrates that interval back to the ordinary evaluator limit. Exact
terminal/tactical/proof examples receive a larger Value-loss weight. Policy is
cross-entropy over legal root moves using the teacher/proven best move.

For a bit-exact cross-language check, use one game record and legal candidates:

```powershell
python -X utf8 training/differential.py --model models/run.rmlp --record sample.rmg --moves H8 H9
cargo run -p rustmoku-data -- model-check --model models/run.rmlp --record sample.rmg --move H8
```

Python deliberately implements Rust's truncation-toward-zero integer division.
The reported Value and each Policy integer must match exactly. This fixture is
numerical contract validation, not a playing-strength claim.

## V1.0 trustworthy data entry

Use the bounded generator wrapper to freeze the actual executable hash and
teacher configuration, publish checked shards, retain completed shards on
restart, audit quality, and create a versioned dataset descriptor:

```powershell
python -X utf8 training/generate.py --engine target/release/rustmoku-data.exe --output target/data-smoke --games 8 --shard-games 4 --seed 7 --depth 1 --nodes 100
python -X utf8 training/train.py --dataset target/data-smoke/dataset.json --output target/smoke.pt --epochs 1 --seed 7
python -X utf8 training/evaluate.py --dataset target/data-smoke/dataset.json --checkpoint target/smoke.pt
python -X utf8 training/export.py --checkpoint target/smoke.pt --dataset target/data-smoke/dataset.json --output target/smoke.rmlp
python -X utf8 training/calibrate.py --dataset target/data-smoke/dataset.json --checkpoint target/smoke.pt --model target/smoke.rmlp
python -X utf8 training/verify_integer.py --engine target/release/rustmoku-data.exe --model target/smoke.rmlp
python -X utf8 -m unittest discover -s training -p 'test_*.py'
```

These tiny budgets intentionally exercise fallback handling, not teacher strength.
For existing files, `training/dataset.py --shard FILE [--shard FILE] --output
DESCRIPTOR.json` records missing teacher provenance as unknown. The descriptor
checks full trajectory content after SHA256 identity checks. Do not concatenate
raw shards with colliding local game IDs. Derived/relabelled/sampled branches
must preserve `lineage_id` and set `parent_lineage_id` to that original root;
unknown ancestry cannot be recovered automatically from sampled positions.

The largest sorted starting canonical opening family is reserved independently
of the split seed when at least three families exist. This is an empirical
starting-position family, not a claim of exhaustive semantic opening taxonomy.
`opening_heldout` and fixed test are confirmation sets, not tuning sets. Audit
separately reports full duplicates and canonical overlaps before/after an
explicit short prefix (default ply 8). Shared starts do not union all games.

Raw dataset V2 retains the V1 16-byte header and 76-byte record prefix; version
is 2 and each record appends `<BBBQQ>`: completed depth, requested depth,
termination (0 completed, 1 work, 2 time, 3 cancelled), total work, requested work
budget. Nineteen 255 bytes encode unknown when migrating old records. V1 remains
readable with unknown fields. Training excludes Fallback and Analysis; an AB
iteration completed before a limit is eligible. Old AB records retain their
known source but unknown depth/work. Exact tactical/terminal labels are separate.

Checkpoint-bound split manifests include fingerprint, seed, grouping/filter
versions and all record indices, and are also saved as immutable `.split.json`.
Evaluate uses this manifest by default and rejects a conflicting optional seed,
changed dataset/provenance, missing manifest or modified partition. Existing
checkpoints without it require a new explicitly identified experiment.

V1 export rejects scale gain errors above 1%, rather than silently changing file
semantics. Float/integer calibration and Rust/Python bit-exact checks are both
required; neither a smoke loss nor successful export promotes a model.

## Recovery and bounded orchestration

`pipeline.py --config FILE --output DIRECTORY` runs finite generation, audit,
training, evaluation, export, calibration, cross-language integer checks,
tactical tests and paired Arena, then rejects/promotes. Example configuration:

```json
{"data_engine":"target/release/rustmoku-data.exe",
 "arena_engine":"target/release/rustmoku-arena.exe",
 "games":8,"epochs":1,"pairs":1,"seed":7,"move_ms":10}
```

Caps are 32 games, 3 epochs and 4 pairs, with a 60-second subprocess timeout per
stage. `--stage NAME` requires prior stages to be complete. Repeat the same
command to verify saved hashes and resume. Changed scripts, inputs, or binaries
require a new output directory. Stage state publication uses atomic replacement;
completed logs and artifacts are hashed. This is a single-owner directory,
not a concurrent job scheduler.

Training `--resume CHECKPOINT --max-steps N --checkpoint-every N` restores Adam,
epoch/order/cursor, Python/Torch/shuffle/CUDA RNG and immutable data/configuration
identity. The CPU one-thread regression compares interrupted and uninterrupted
weights, optimizer and RNG exactly. GPU bit-exact execution is not established.
Checkpoint storage is bounded to 128 MiB, loaded with `weights_only=True`.

`rustmoku-data proof --book FILE --output FILE.rmd` exports only independently
verified strategies; `import_proof.py` creates the lineage-aware descriptor.
OR policy means a proven winning choice; absent AND policy does not imply every
defense is equally optimal. AtMost distance is not global shortest distance.
Default `--max-proof-fraction 0.25` limits proven examples in training sampling.
Conflicting exact outcomes stop data acceptance rather than averaging truth.

`apps/rustmoku-arena/experiment.py --config FILE --output DIRECTORY` accepts a
JSON config with `arena`, `arguments`, `max_pairs`, `suite_role`, `stop_rule`,
`sprt`, and `game_timeout_seconds`. `arguments` is an argument array, never shell
text. Fixed work uses `--nodes`; fixed time uses `--move-ms` and optionally
`--clock-ms`/`--increment-ms`. Supply repeated `--opening-record` paths for an
independent suite; twelve built-in openings are only an initial diagnostic set.
`--a-external PATH` plus repeated `--a-external-arg ARG` uses the bounded pbrain
adapter (likewise B). Only 15x15 Freestyle is supported. The supported protocol
subset follows the [official description](https://plastovicka.github.io/protocl2en.htm).

Pentanomial profile likelihood provides descriptive paired Elo and generalized
SPRT; its composite-model asymptotics are not an exact finite-sample error proof.
The conservative fixed-sample Hoeffding interval treats distinct opening clusters
as independent bounded pair scores. Repeated deterministic starts count once.
Smoke and tuning results cannot promote. `promote.py` requires independent
fixed-time, fixed-sample evidence, current candidate/champion hashes and no
confirmation-start overlap with the supplied training corpus. Independence of
the opening construction and undisclosed prior tuning is still an experimental
responsibility; hashes alone cannot certify it. Previous champions remain stored.

Research Arena controls include `--a-disable rfp,lmr` (or `all`) and
`--a-interior-vcf 5:240:960` (plies:per-probe-work:per-search-work), likewise B.
The proof experiment is disabled with multiple workers and remains off by
default. These controls are not Native settings or calibrated model profiles.

## Work package 1: experiment identity and publication

`rustmoku-arena --describe [OPTIONS]` validates files and prints schema-2 JSON
without playing. Rust alone resolves evaluator/model selection, threads, TT,
proof/selectivity settings, clock/work limits, opening identities and SHA256
inputs. Compatible `--a-evaluator learned --a-model FILE` works in either order;
Pattern/Classical plus a model, conflicting external selection and duplicate
single-value options are errors. B has the same independent checks. Actual
games emit `EFFECTIVE_CONFIG` JSON on stderr. The runner compares it with
preflight before accepting a leg and binds every completed event to the
experiment and effective configuration. Old version-1 experiments are not
promotion evidence and cannot be silently resumed as version 2.

Export now requires `--dataset` to check the checkpoint-bound corpus and split.
It writes an immutable `.rmlp.export.json` alongside the unchanged V1 model
format. Calibration and integer verification write `.calibration.json` and
`.integer.json` receipts tied to that exact export/model hash; the second
completed check publishes `.evidence.json`. Receipts record checkpoint/data/
shard/split hashes, architecture, score contract, calibration thresholds and
metrics, checker executable and producer script hashes. Confirmation experiments
freeze this complete input set. Old models still load for runtime diagnostics,
but cannot promote without this evidence. Preserve the referenced original
checkpoint, corpus and receipts when archiving a champion; copying model bytes
alone does not preserve a reproducible training experiment.

`promote.py --dataset` is now optional and only asserts equality with the bound
corpus. It cannot select a different corpus for overlap checks. The experiment
configuration's `promotion_kind` defaults to `model`: actual A/B engine, search
profile, threads, TT and reset/book settings must agree. Explicit
`engine-model-profile` promotion can compare different profiles, but publishes
the actual winning `competition_identity` and match limits with the model.
Future player B must match the recorded current champion combination. The
atomic champion pointer retains the previous record. A model hash alone is not
the identity of a champion with different search parameters.

The external adapter sends CRLF and accepts LF/CRLF/CR, including split buffers.
It initializes `timeout_match`, sends real `time_left` before each request, and
reports the true turn cap as `timeout_turn`; the internal manager's 95% soft
allocation is not a match clock. External ERROR details survive adjudication.
The generic protocol has no portable thread control: external threads and TT
are `unavailable`, and internal thread/TT/proof flags on external players are
rejected. Optional `--b-external-memory-bytes N` sends advisory `max_memory`;
it is explicitly not OS-enforced memory. File arguments, including `--key=file`,
are hashed automatically; declare implicit network/configuration files using
repeatable `--b-external-input FILE` (or A). These declarations cannot discover
hidden dependencies loaded by an arbitrary executable; preserve the exact
external launch environment for a formal comparison.

All run/dataset/shard/split/pipeline/experiment JSON manifests use the same
no-clobber publisher: same-directory temporary write, flush/fsync, parse check,
then atomic hard-link publication. NTFS or a Linux filesystem with hard links
is required; unsupported filesystems fail rather than fall back to overwriting.
Unpublished `.partial` files can be ignored on retry. A truncated **published**
file is reported as corruption requiring explicit recovery; a complete different
manifest is an identity mismatch. Neither is silently replaced. Fault tests
cover write interruption, pre/post publication, abrupt process exit and a
concurrent publisher. Windows has no directory-fsync API exposed by this Python
implementation; post-crash damage is detected rather than repaired by guessing.

Proof bundle merge resolves shard paths in the source descriptor directory
before publishing the merged bundle. Failed imports never overwrite existing
shards. Run formal regressions after building their bounded subprocess fixtures:

```powershell
cargo build --release -p rustmoku-arena -p rustmoku-data
python -X utf8 -m unittest discover -s training -p 'test_*.py'
python -X utf8 -m unittest discover -s apps/rustmoku-arena -p 'test_*.py'
```

Synthetic promotion outcomes in these tests validate rejection and publication
mechanics only. They are never included in Arena statistics or strength claims.

## MixLite V3 scalar research pipeline

The dedicated CPU/QAT pipeline uses whole-trajectory splits, train-only scale
selection, deterministic post-split D4 augmentation, WDL/value and contextual
policy loss (including observed comparison masks), bounded steps/time, resumable
optimizer state, and dataset/split validation on resume and export.

```powershell
python training/mixlite.py train --dataset DATASET --checkpoint v3.pt --steps 8 --max-seconds 60
python training/mixlite.py export --checkpoint v3.pt --dataset DATASET --model v3.rml
python training/mixlite.py verify --engine target/debug/rustmoku-data.exe --model v3.rml
```

V3 has independent magic/version/architecture (`RMLPV003`, 3, 4) and shares the
rational Q15 score contract. Header `<8sHHIHHiii>` declares features=65536,
width=32, contract=2, value divisor, policy divisor and frozen score scale.
Tensor order: int8 embeddings, 3 occupancy rows, 8x160 mixing; eight bounded i32
biases; int16 3x8 WDL, 32 policy and eight contextual policy weights. Local and
context activations clip to [0,255]; all signed divisions truncate toward zero.
WDL evidence is `max(0, dot/value_divisor)+1`. Q15 win-minus-loss is converted
through the bounded rational value contract. The independent integer reference
rebuilds the board; Rust uses bounded incremental updates.

V3 formal export, calibration and integer evidence now use the common
architecture-aware admission path. `mixlite.py verify` remains a research-only
receipt; formal verification uses `verify_integer.py`. A portable V3 model
requires Python integer == Rust scalar == Rust auto (30 comparisons), with
architecture, CPU availability and exercised backend recorded. An explicitly
AVX2 competition target additionally requires the separate `simd-avx2` receipt:

```powershell
python training/verify_integer.py --engine target/debug/rustmoku-data.exe --model v3.rml
python training/verify_integer.py --engine target/debug/rustmoku-data.exe --model v3.rml --target-backend-policy avx2
```

Neither receipt supplies playing-strength evidence. Promotion still requires
frozen dataset/split/model provenance and independent Arena evidence. ARM and
non-AVX2 machines can produce valid portable semantic receipts.

`mixlite_production.py` supplies batched float32 QAT with CPU/CUDA device,
batch size, steps/epochs, checkpoint/resume, soft/ranking and outcome targets.
The cached D4 group index tensors move with the model. `--mining-every 0`
disables online hard-example diagnostics; N>0 samples every Nth step, avoiding
mandatory GPU-to-CPU policy/WDL transfers on ordinary steps. Presets expose
`exact_weight`, `hard_weight`, `outcome_weight`, and `mining_every`; these are
frozen on resume. Exact negative labels are tagged `exact-forced-loss`, not
resistance ties. A teacher comparison mask never asserts production candidate
coverage. Train-only scale selection samples deterministically across the
training partition. Feature construction still happens on CPU; CUDA throughput
and large-scale convergence remain unmeasured.

`scripts/train-production.ps1` offers smoke/pilot/serious/large configurations,
CUDA/CPU, batch/scale overrides and resume. `scripts/arena-confirm.ps1` freezes
independent competition evidence; neither training nor verification promotes a
model automatically. `mixlite.py` remains the independent scalar training/export
oracle; V1/V2 compatibility is retained.

Teacher JSON v4 declares root/descendant universes, leaf policy, search domain,
selectivity and score reference scale. Practical distillation is the default;
all-descendant oracle analysis is explicit. `DomainExact` never means solved
Gomoku. Exploration temperatures use reference units and comparisons retain the
actual raw-unit temperature for exact distribution validation.

## Offline search tuning and CI executables

See [Search tuning](../docs/SEARCH_TUNING.md) for frozen independent suites,
paired feature ablations, staged SPSA, deterministic resume and confirmation.
These tools do not automatically enable research features or promote a model.

Integration tests resolve `RUSTMOKU_DATA_EXE` / `RUSTMOKU_ARENA_EXE` when explicitly
set; otherwise they require `target/release/rustmoku-data[.exe]` and
`target/release/rustmoku-arena[.exe]`. Missing files fail clearly. There is no
fallback to stale debug binaries and tests do not compile another build profile.
