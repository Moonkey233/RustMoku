# Offline intelligence development interface

Run these commands from the repository root. W9 provides bounded offline tooling; million-node throughput and a production
proof farm have not been validated.
OpeningDatabase is empirical. GameRecord is chronological. Only a fresh parsed
and independently verified ProofBook has runtime exact authority.

## Build and bounded opening generation

```powershell
cargo build --release -p rustmoku-book -p rustmoku-solver -p rustmoku-data
scripts/build-opening.ps1 -Record root.rmg -Output opening.rmopen -MaxOpeningPlies 2 -TopK 2 -Depth 1 -Nodes 2000 -MaxPositions 4
scripts/build-opening.ps1 -Record root.rmg -Output opening.rmopen -MaxOpeningPlies 2 -TopK 2 -Depth 1 -Nodes 2000 -MaxPositions 4 -Resume
target/release/rustmoku-book inspect --output opening.rmopen
target/release/rustmoku-book query --output opening.rmopen --record root.rmg
```

`-Model` and `-Profile` select the frozen evaluator/profile. The script hashes
the actual executable before/after generation; the database binds shared
Core/Engine/SIMD source, compiler, target and build configuration identity,
model fingerprint, effective profile,
generation settings and canonical root. Incompatible resume fails. The opening
builder normalizes each offline position before analysis and reuses the stored
canonical ranking on both fresh and resumed paths. A two-ply fixture verifies
byte-identical build/resume. Close scores widen to TopK; immediate winning and
blocking points are retained. MaxPositions bounds new work per invocation and
MaxFrontier bounds the queue. It currently supports one worker and at most
100,000 empirical entries; increasing those limits is separate scalability work.
If the frontier cap is reached, resume with a larger cap to progress beyond it.

## Disk proof baseline

One-minute work slice (verification can add time, as described below):

```powershell
scripts/solve-proof.ps1 -Record root.rmg -Attacker black -Checkpoint proof.db -Output proof.rmp -Nodes 1000 -Seconds 60 -SqliteCacheMiB 32 -DiskMiB 1024
```

Manual one-hour and overnight examples, **not executed during development**:

```powershell
scripts/solve-proof.ps1 -Record root.rmg -Attacker black -Checkpoint proof.db -Output proof.rmp -Nodes 1000000 -Seconds 3600 -SqliteCacheMiB 64 -DiskMiB 16384 -Resume
scripts/solve-proof.ps1 -Record root.rmg -Attacker black -Checkpoint proof.db -Output proof.rmp -Nodes 10000000 -Seconds 28800 -SqliteCacheMiB 64 -DiskMiB 65536 -Resume
```

The database freezes the executable SHA256, replayed root, attacker, rules and
root ply, additional proof plies and tactical leaf budgets; resource budgets may change on resume. A matching small
`proof.db.config.json` supports frontier creation. SQLite stores stable NodeIds,
canonical exact/transposition keys, chronological replay moves, PN/DN, outcome,
legal edges and a persisted dirty/frontier index. Transactions preserve logical
checkpoints without serializing a full resident Position tree. Working outcome
flags remain untrusted, including refutations. Resource stops return Unknown.

All legal children are inserted before aggregation. OR wins require one proven
child; AND wins require every legal reply. The dual witness is replayed for a
refutation. Final win export streams a candidate ProofBook to a temporary path,
then invokes the original native fresh parser/verifier before publication. No
working database flag or learned prediction bypasses that verifier.

Current limits are important:

- The new path has no 100k explored-node ceiling. The original native in-memory
  solver retains its separate 100k checkpoint ceiling.
- SqliteCacheMiB (legacy RamMiB alias) bounds the SQLite page cache, **not whole-process RSS**.
  Python, native replay and at most one board-depth verification stack add fixed
  overhead. DiskMiB reserves half the database quota for rollback pages; proof
  exports, JSON artifacts and multiple worker directories need separate space.
- Nodes counts PN expansions plus tactical probe search visits, not every native
  replay/D4 operation. MaxAdditionalPlies is relative to the frozen proof root,
  never absolute game ply. Tactical plies are capped by the remaining horizon.
- Seconds is a cooperative search-phase budget checked between expansions/native
  calls, not a hard whole-job deadline. Witness verification has separate node,
  tactical-work and wall allowances. Native leaf certificate replay and final
  fresh verification are additionally bounded by their declared verifier limits.
  A verifier cannot borrow an untrusted database's larger probe allowance.
- Exact leaves run terminal/immediate, then VCF, then VCT before all-legal PN
  expansion. Use -VcfPlies/-VcfNodes/-VctPlies/-VctNodes to configure them; these
  semantics are frozen on resume. Exhaustion/NotProven always remains Unknown.
- Reports expose native calls, replayed plies, generated children, request/response
  bytes, native roundtrip/JSON time, SQLite transactions, witness nodes/tactical
  work and leaf hits. No whole-process RSS enforcement or measurement is claimed.
- A release 16-expansion baseline measured 0.347s total, 0.030s native roundtrip,
  0.0023s JSON parsing and 3810 generated children. Native/JSON did not dominate;
  no new IPC protocol was justified. This is a diagnostic, not scale evidence.
- Only tiny fixtures have run. Million-node throughput, power-loss behavior on
  target filesystems and multi-machine operational recovery are not established.

## Deterministic process shards

```powershell
scripts/solve-proof-batch.ps1 -Command frontier -Checkpoint proof.db -Config proof.db.config.json -Manifest frontier.json -Shards 2 -Limit 16
scripts/solve-proof-batch.ps1 -Command solve-batch -Manifest frontier.json -Output worker0 -Shard 0 -Workers 2 -Nodes 1000 -Seconds 60
scripts/solve-proof-batch.ps1 -Command solve-batch -Manifest frontier.json -Output worker1 -Shard 1 -Workers 2 -Nodes 1000 -Seconds 60
scripts/solve-proof-batch.ps1 -Command merge -Manifest frontier.json -Checkpoint proof.db -Inputs worker1,worker0 -Nodes 10000 -Seconds 60
```

Add `-Resume` to solve-batch to continue compatible per-job checkpoints. Budgets
are **per job**, so total cost multiplies with jobs/workers. Each worker owns a
native process and independent SQLite connection; no shared mutable PN tree is
introduced. Jobs are identified by the full canonical key and deterministic
SHA256 shard assignment. Artifacts contain portable move/key/config data and can
be copied with their database to a machine running the matching frozen binary.
There is no network scheduler. Stop writers before copying or merging artifacts.
Merge sorts keys independently of input order and freshly replays each solved
witness. Unknown artifacts never update parent proof/disproof; their databases
remain available for resume. Conflicting duplicate job artifacts are rejected.

## Verified-proof training and composition

```powershell
python training/import_proof.py --engine target/release/rustmoku-data.exe --book proof.rmp --output proof-data --base-dataset broad-data/dataset.json --max-positions 1000
scripts/train-production.ps1 -Preset smoke -Dataset proof-data/dataset.json -OutputDirectory training-smoke -Device cpu
```

The importer retains verified proof lineage. For explicit weighted composition,
import proofs without `--base-dataset`, then write a composition config:

```json
{"version":1,"seed":1,"sources":[
  {"kind":"selfplay","path":"broad-data/dataset.json","weight":1},
  {"kind":"reanalysis","path":"deep-data/dataset.json","weight":1},
  {"kind":"verified-proof","path":"proof-data/dataset.json","weight":0.5,"keep":0.25},
  {"kind":"tactical","path":"hard-data/dataset.json","weight":2},
  {"kind":"opening","path":"opening-data/dataset.json","weight":1}
]}
```

Use only existing input paths; optional categories may be omitted. Paths resolve
relative to the composition config. Source datasets and shard provenance are
frozen by hashes, with model/profile/executable identities retained in teacher
metadata. Original labels, proof origin, outcomes and comparisons are preserved;
conflicting comparisons reject. Exact opening labels reject instead of being
relabelled. Ordinary AB records never acquire exact=true. Source bundles must
not themselves be composed.

```powershell
scripts/compose-training-data.ps1 -Config composition.json -Output mixed-data
scripts/train-production.ps1 -Preset smoke -Dataset mixed-data/dataset.json -OutputDirectory mixed-smoke -Device cpu
```

Repeating composition with identical inputs is an idempotent resume; changed
configuration cannot overwrite a published dataset. Splitting retains original
trajectory/lineage metadata before canonical deduplication and deterministic
proof subsampling are applied to the train partition. Validation/test partitions
are not sampled using training controls. Production loss multiplies the retained
record's sample weight; proof exact_weight remains a separate trainer setting.
Large-scale composition and CUDA training have not been exercised. OpeningDatabase examples and starts are now available:

```powershell
python training/import_opening.py --engine target/release/rustmoku-data.exe --opening-db opening.rmopen --output opening-data --max-positions 1000
scripts/train-production.ps1 -Preset smoke -OpeningDb opening.rmopen -OutputDirectory opening-smoke -Device cpu
```

Selfplay samples canonical 2..16-ply DB positions with deterministic game-ID
seeds. It imports positions only, not stored scores or BookMove authority. Core
constructs a legal setup replay; this is not historical played-game reconstruction.
The opening DB hash and starting-position family are retained in provenance.
Serious/large presets use the already supported built-in opening suite and zero
random prefix; an explicit opening DB replaces that suite. Random prefixes remain
an available generation ablation. Native/Arena BookMove configuration is still
pending, independently of these training interfaces.

## Runtime empirical opening use

Build book, Native and Arena from the same source/toolchain/build configuration.
`rustmoku-book build-identity` prints their shared engine identity; it is distinct
from each executable's SHA256. Old books using the book executable hash must be
rebuilt. Model/profile/build mismatch rejects loading; no identity-independent
fallback is enabled. Source/feature/target changes deliberately invalidate books.

Native has an **Empirical opening database (not proof)** section with a path,
OrderOnly/BookMove, load/disable and status. Reconfiguring the engine or changing
the evaluator disables the loaded book until it is loaded and validated again.

```powershell
.\target\release\rustmoku-arena.exe --describe --a-opening-db opening.rmopen --a-opening-policy order-only --b-opening-db opening.rmopen --b-opening-policy book-move
```

Supply matching `--a-profile`/`--b-profile` strings and models as needed. Arena
freezes the exact loaded bytes (SHA256), policy, model fingerprint, profile,
generation and shared engine identity in EFFECTIVE_CONFIG and experiment inputs.
OrderOnly still searches. BookMove reports OpeningBook, has no proof or TT Exact
value, and never overrides immediate wins, exact losses or a unique forced block.

SQLite schema/config version 2 rejects old incompatible checkpoints rather than
silently reinterpreting their depth or leaf semantics. Budget increases are
allowed on resume; semantic horizon/tactical limits are not changed silently.
