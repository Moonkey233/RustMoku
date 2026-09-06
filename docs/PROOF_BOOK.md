# RustMoku Freestyle Proof Book

RustMoku V0.11 separates three concepts:

- an opening suite is an empirical list of starts;
- a solver checkpoint is resumable, untrusted working state;
- a Proof Book is a persisted winning strategy.

Only an independently verified Proof Book may affect runtime search.

## Identity and proof semantics

Every entry is keyed by attacker plus Core's collision-free 58-byte D4 canonical
position key. The key contains all 225 cells and side to move; the format fixes
rules to Freestyle. Stored moves use canonical orientation and are transformed
back with the inverse symmetry, then checked for legality.

Attacker nodes are OR nodes and store one proven action. Defender nodes are AND
nodes and require the verifier to visit every legal reply. Terminal facts and
freshly rerun Immediate/VCF/VCT proof leaves are the only leaf evidence. A
bounded unsuccessful tactical search is never a refutation.

Distances are tagged `Exact` or `AtMost`. A general strategy and a VCF/VCT leaf
establish a win within the stored number of plies but do not claim the globally
shortest forced win. Runtime mate-like scores derived from `AtMost` are lower
bounds and retain the explicit distance tag.

## Binary format version 1

All integers are unsigned little-endian. No raw Rust layout is serialized.

```text
8 bytes   magic "RMPBOOK1"
u16       version = 1
u8        rules = 0 (Freestyle)
u32       root count
u32       entry count

root[]:
  u8      attacker (0 Black, 1 White)
  u16     ordered move count
  u8[]    validated move indices
  [u8;58] canonical root key

entry[] sorted strictly by (attacker, canonical key):
  u8      attacker
  [u8;58] canonical position key
  u8      distance tag (0 Exact, 1 AtMost)
  u8      distance plies
  u8      action tag
  ...     action payload
```

Action tags are attacker move (canonical move byte), defender-all, immediate
leaf, VCF leaf, or VCT leaf. Tactical leaves encode an optional canonical first
move, maximum plies, and `u64` node limit so a fresh verifier can reproduce the
proof rather than trusting a serialized outcome or PV.

The parser rejects unknown tags, invalid packed cells/padding, invalid moves,
duplicates or unsorted entries, excessive counts, truncation, and trailing
bytes before allocation or verification. Verification additionally rejects
illegal roots/transitions, key or symmetry mismatches, missing defender
children, wrong tactical moves/distances, inconsistent proof distances, cycles,
and unreachable entries.

## Runtime

`AlphaBetaEngine` accepts only `Arc<VerifiedProofBook>`. Positive-depth root
order is terminal, Immediate, Proof Book, VCF, VCT, Alpha-Beta. Book lookup does
not occur recursively, does not run for zero-depth analysis, and never stores a
result in the ordinary transposition table.
