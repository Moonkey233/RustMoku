# RustMoku V0.12 learned local-pattern model

## Architecture and semantics

Each directional window uses the existing u16 `LineKey`: offsets
`[-4,-3,-2,-1,+1,+2,+3,+4]`, two bits per cell, with the candidate center
omitted. The complete 65,536-key space indexes a direct i16 table of 16-value
embeddings; there are no hash collisions.

At initialization, all 225 x 4 keys are made relative to Black and White and
summed into two i32 accumulators. An accepted move changes at most 32 keys in
`PatternState`; its opaque fixed-capacity delta updates both accumulators by
subtracting old and adding new embeddings. Every Lazy-SMP worker initializes its
own mutable accumulator while sharing immutable weights through `Arc`.

The Value head is an i64 dot product plus bias divided with Rust integer
truncation toward zero, then clamped to +/-10,000,000. It selects the accumulator
for the side to move. This is an ordinary heuristic score, never proof evidence.

For one legal candidate, Policy sums only its four side-relative embeddings,
dots an i16 Policy head in i64, divides and clamps to i16 range. It is evaluated
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
