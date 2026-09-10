# RustMoku V0.12 learned local-pattern model

## Architecture and semantics

Each directional window uses the existing u16 `LineKey`: offsets
`[-4,-3,-2,-1,+1,+2,+3,+4]`, two bits per cell, with the candidate center
omitted. The complete 65,536-key space indexes a direct i16 table of 16-value
embeddings; there are no hash collisions.

At initialization, all 225 x 4 keys are made relative to Black and White and
compiled at load time into two i64 scalar tables (Value and Policy).
Their per-side sums are maintained in two i64 accumulators. An accepted move changes at most 32 keys in
`PatternState`; its opaque fixed-capacity delta updates both accumulators by
subtracting old and adding new scalar Value entries. Every Lazy-SMP worker initializes its
own mutable accumulator while sharing immutable weights through `Arc`.

Each Value table entry is the complete embedding/head dot product, without
intermediate division or clamp. The accumulated scalar plus bias is divided with Rust integer
truncation toward zero, then clamped to +/-10,000,000. It selects the accumulator
for the side to move. This is an ordinary heuristic score, never proof evidence.

For one legal candidate, Policy sums its four side-relative precompiled i64
Policy table entries, then divides and clamps to i16 range. It is evaluated
only for generated candidates. Ordering remains:

```text
exact/tactical class > legal TT preference > learned Policy
> countermove/killer/history > structural/canonical ties
```

Policy does not alter pruning, LMR, LMP, extensions, or tactical solving.
PatternEvaluator remains the built-in default and Native fallback. Any evaluator
replacement clears the ordinary TT.

## Binary format version 1

All fields are fixed-width little-endian values; no Rust layout is serialized.

```text
[u8;8] magic = "RMLPV001"
u16     format version = 1
u16     architecture id = 1
u32     feature count = 65536
u16     hidden width = 16
u16     flags = 0
i32     Value integer divisor (> 0)
i64     Value bias in quantized dot-product units
i32     Policy integer divisor (> 0)
u32     embedding value count = 65536 * 16
u16     Value-head count = 16
u16     Policy-head count = 16
i16[]   embeddings, key-major then dimension
i16[16] Value head
i16[16] Policy head
```

The parser caps files at 4 MiB and requires exact dimensions and payload length.
It rejects bad magic/version/flags, nonpositive divisors, truncation, trailing
data, overflowed counts and a bias that would violate worst-case i64 dot-product
bounds. The maximum accumulator magnitude is `225 * 4 * 32768`, within i32;
Value and Policy dot products are accumulated in i64.

Training checkpoints are not production models. `training/export.py` quantizes
the shared embedding and both heads, folds their float scales into the stored
integer divisors, and calibrates normalized Value output to RustMoku evaluator
units. See [`training/README.md`](../training/README.md).

## Runtime selection and measurement

Native starts with Pattern, can load a `.rmlp` file, and safely returns to Pattern
with an error message if parsing fails. Arena accepts independent
`--a-evaluator learned --a-model FILE` and B equivalents. The fixed-position
benchmark accepts `--evaluator learned --model FILE` and an optional fixed
`--nodes N` work cap; the hot-path benchmark
accepts `--model FILE` for scalar Value/Policy and reversible-update timing.

V0.12 keeps the measured, deterministic Safe Rust scalar path. It introduces no
unsafe intrinsics or SIMD dependency; a future SIMD path requires a clear
same-model benchmark benefit and bit-exact differential coverage.

## V1.0 calibration and equivalent compilation

V1 file bytes and loader semantics are unchanged. The float ordinary Value unit
is 10,000,000 engine score units; Policy logits use 4096 integer units. Export
rejects divisors whose gain differs by more than 1% from that contract, including
1024 x 1024 Value scales. This gate is separate from weight rounding error.
`training/calibrate.py` measures clamped float versus integer absolute/relative
error, significant sign mismatches, ordering and saturation on checkpoint-bound
validation records. An unavailable ordering comparison is reported as null.

The runtime compiles `V[k] = sum_d E[k,d]*Hvalue[d]` and an analogous Policy
table. Products and sums remain i64. Each entry has magnitude at most
`16*32768^2`, and 900 entries fit in i64; the existing bias bound remains valid.
Only the final sum receives bias, division toward zero and output clamp.
The two tables add 1 MiB shared immutable memory; serialized weights are retained
for exact V1 round-trip output. No nonlinear expression capacity is added.

## Experimental nonlinear reference

Feature `experimental-v2` loads `RMLREF02`, architecture 2, width 8, in a bounded
offline f64 reference. Four directional embeddings and a three-state center
occupancy embedding are summed at each point, followed by ReLU. A global mean
Value head and local Policy head operate on these activated vectors. This adds
actual interactions between directional features; increasing V1 width would not.
`training/nonlinear.py` supplies the matching float64 PyTorch reference.

The reference incrementally maintains feature keys and center occupancy from
PatternDelta, with reversible undo, then deliberately recomputes inference.
It is not a fast evaluator. Weight storage is approximately 4.2 MiB; inference
is O(225 * 4 * 8), and an optimized implementation would need bounded affected
point activation deltas plus a reversible global sum. Quantization, QAT,
distillation, training comparison and production inference are not implemented.

The intended ablation order is folded V1, directional ReLU, center occupancy,
then global grouping. Keep dataset/split and training budget fixed and report
heldout error, parameter/storage cost, inference cost, and independent fixed-time
Arena before adopting any variant. One Rust/Python numerical fixture agreeing
to about 1e-17 only validates the float reference, not strength or calibration.
