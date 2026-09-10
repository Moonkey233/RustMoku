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
python -X utf8 training/export.py --checkpoint models/run.pt --output models/run.rmlp
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
python -X utf8 training/export.py --checkpoint target/smoke.pt --output target/smoke.rmlp
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
