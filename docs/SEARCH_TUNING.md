# Offline SearchProfile tuning

This is experiment infrastructure, not a claim that any research flag improves
strength. Search and time-management stages are separate. All search ablations
and numeric stages use fixed work, identical openings and reversed colors;
final model/profile confirmation uses an independent frozen fixed-time suite.
Nothing is tuned inside recursive search.

## Freeze a base and prepare explicit suites

```powershell
cargo build --release -p rustmoku-arena -p rustmoku-data
$effective = & .\target\release\rustmoku-arena.exe --describe --a-model .\models\candidate.rmlp --b-model .\models\candidate.rmlp | ConvertFrom-Json
$effective.players[0].profile.parameters | Set-Content -Encoding utf8 .\base.profile
```

Use chronological `.rmg` records for each suite. The tuner asks the native Arena
for canonical D4 identities and rejects duplicate tuning roots and any overlap
with confirmation roots. Explicitly choose the number of independent pairs;
there is no implicit built-in promotion suite. Paths in configs resolve from
the working directory, normally the repository root.

Example `tuning.json` (replace input paths with real files):

```json
{
  "version": 1, "mode": "ablation", "seed": 1,
  "arena": "target/release/rustmoku-arena.exe",
  "model": "models/candidate.rmlp", "base_profile": "base.profile",
  "nodes": 10000, "depth": 64, "pairs": 2,
  "tuning_openings": ["openings/train-1.rmg", "openings/train-2.rmg"],
  "confirmation_openings": ["openings/holdout-1.rmg", "openings/holdout-2.rmg"],
  "player_options": {"threads": 1, "tt-mib": 64},
  "features": ["lmr_v2", "improving", "iid"],
  "game_timeout_seconds": 600
}
```

```powershell
.\scripts\tune-search.ps1 -Config .\tuning.json -OutputDirectory .\runs\ablation
# Only the explicit -Run starts games. Repeat the same command to resume.
.\scripts\tune-search.ps1 -Config .\tuning.json -OutputDirectory .\runs\ablation -Run
```

Each listed feature gets a separate enabled-vs-disabled experiment against the
same frozen context. Supported switches: lmr_v2, lmr_v2_policy, improving, iid,
policy_lmr, policy_pruning, singular, competitive_tt, null_move, qsearch_threes,
interior_vcf, interior_vct, calibrated_probcut, adaptive_root_candidates.
`lmr_v2_policy` requires a deliberately enabled LMR V2 base; changing its dormant
bit with LMR V2 disabled would not test anything. Interior probes require explicit
`feature_values` such as `{"interior_vcf":"6:64:512"}`. ProbCut requires a valid
serialized `calibrated_probcut` calibration; native model/profile admission still
applies. No missing calibration is invented and nothing is enabled by default.

## Small numeric SPSA stages

Change mode to `spsa`, remove `features`, and add for example:

```json
"stage": "futility",
"steps": 100,
"bounds": {"futility_base": [300, 1000], "futility_depth": [600, 1800]},
"spsa": {"a": 0.01, "c": 0.1}
```

Available stages and parameter names are explicit in `training/tune_search.py`:
LMR, improving, LMP, futility/RFP, razor, history, policy, null and time. Choose
bounds respecting Rust's validation and coupled constraints (e.g. null reduction
must remain below minimum depth). Tune only active mechanisms in the base.
The normalized finite-difference update uses deterministic hash-derived signs,
integer serialized candidates, decreasing gain/radius, and bounded parameters.
It is a noisy optimizer, not a strength test or automatic production-default
selector. No more than one small stage is accepted per run.

A time stage is a separate SPSA experiment: `stage="time"`, `move_ms`, no `nodes`.
Do not mix time-management parameters into fixed-work search tuning.

The root manifest binds the exact engine, model, base profile, suite bytes and
canonical identities, runner inputs, seed and optimization controls. Every duel
has immutable A/B profiles and its own existing Arena journal. Interrupted legs
resume; completed legs are not played again. States and candidate profiles are
numbered and never overwritten. Changing any frozen input requires a new output
directory. Expensive evidence is retained even if a later candidate is rejected.
Reports retain pentanomial counts, scores, completed depth, work/time rate and
per-player tactical/feature counters. Work/sec is telemetry, not an Elo metric.

## Independent fixed-time confirmation

Create a normal `training/confirm.py` config with `suite_role="confirmation"`,
`stop_rule="fixed_pairs"`, explicit holdout opening records, A/B model/profile
arguments and `--move-ms` (no `--nodes`). Add:

```json
"tuning_manifest": "runs/ablation/manifest.json"
```

This checks that confirmation roots belong to the predeclared independent holdout
and not to tuning. The manifest itself becomes a frozen confirmation input.
Learned confirmation still requires the real export/calibration/integer receipts;
synthetic smoke evidence cannot promote a champion.

```powershell
.\scripts\arena-confirm.ps1 -Config .\confirmation.json -OutputDirectory .\runs\confirmation
```

Candidate recall can be collected with broad `rustmoku-data analyze` output.
`production_recall` reports best/canonical-best inclusion, top1/3/8 hit fractions,
Chebyshev distance of a missed canonical best from existing stones, ply and phase.
Fractions use min(k, legal roots), with move-index tie breaking. Incomplete common
horizons report null. Radius-two omission remains a policy choice, never proof.

## Pre-training operating commands

```powershell
.\scripts\train-production.ps1 -Preset pilot -Device cuda -BatchSize 64 -OutputDirectory .\runs\pilot
.\scripts\train-production.ps1 -Preset serious -Device cuda -BatchSize 256 -OutputDirectory .\runs\serious
.\scripts\train-production.ps1 -Preset large -Device cuda -BatchSize 512 -Dataset .\datasets\mixed\dataset.json -OutputDirectory .\runs\large
```

`-TeacherModel` and `-TeacherProfile` accept the intended frozen teacher;
`-Resume` takes the existing checkpoint. No serious/large run was executed during
this engineering iteration. See OFFLINE_INTELLIGENCE.md for opening/proof jobs.
