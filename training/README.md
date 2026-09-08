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

Splitting is always by whole `game_id` before sampling or augmentation. D4
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
