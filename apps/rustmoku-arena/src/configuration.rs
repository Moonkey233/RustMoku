//! One canonical description of the options actually used by the Arena.
use super::{EvaluatorConfig, Options, PlayerConfig};
use rustmoku_core::{CanonicalPosition, Game, GameStatus, OPENINGS};
use rustmoku_engine::{Evaluator, RuntimeEvaluator, ScoreContract};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

fn absolute_file(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = std::path::absolute(path)?;
    if !path.is_file() {
        return Err(format!("not a regular input file: {}", path.display()).into());
    }
    Ok(path)
}

fn hash_file(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn input(path: &Path, inputs: &mut BTreeMap<PathBuf, String>) -> Result<PathBuf, Box<dyn Error>> {
    let path = absolute_file(path)?;
    inputs.insert(path.clone(), hash_file(&path)?);
    Ok(path)
}

fn player(
    config: &mut PlayerConfig,
    inputs: &mut BTreeMap<PathBuf, String>,
) -> Result<Value, Box<dyn Error>> {
    if let EvaluatorConfig::External(path) = &config.evaluator {
        let executable = input(path, inputs)?;
        for path in &config.external_inputs {
            input(path, inputs)?;
        }
        // File arguments (including --key=file) are discoverable dependencies.
        // Implicit weights/configuration must be declared with external-input.
        for arg in &config.external_args {
            let value = arg.split_once('=').map_or(arg.as_str(), |(_, value)| value);
            if Path::new(value).is_file() {
                input(Path::new(value), inputs)?;
            }
        }
        return Ok(json!({"evaluator": "external", "executable": executable,
            "executable_sha256": inputs[&executable], "arguments": config.external_args,
            "model": null, "threads": null, "tt_mib": null, "profile": null,
            "resources": {"threads": "unavailable", "memory": {
                "requested_bytes": config.external_memory, "enforcement":
                    if config.external_memory.is_some() { "advisory-protocol-only" } else { "unavailable" }}},
            "dependency_policy": "argument-files-and-explicit-external-inputs"}));
    }
    let (evaluator, model) = match &config.evaluator {
        EvaluatorConfig::Pattern => ("pattern", Value::Null),
        EvaluatorConfig::Classical => ("classical", Value::Null),
        EvaluatorConfig::Learned(path) => {
            let path = absolute_file(path)?;
            let mut bytes = Vec::new();
            File::open(&path)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            let model = RuntimeEvaluator::from_model_bytes(&bytes)?;
            let hash = format!("{:x}", Sha256::digest(&bytes));
            let (metadata, score_scale) = match &model {
                RuntimeEvaluator::Learned(model) => (model.model().metadata(), None),
                RuntimeEvaluator::Nonlinear(model) => {
                    (model.model().metadata(), Some(model.model().score_scale()))
                }
                RuntimeEvaluator::MixLite(model) => {
                    (model.model().metadata(), Some(model.model().score_scale()))
                }
                RuntimeEvaluator::Pattern => unreachable!("model reader cannot select Pattern"),
            };
            inputs.insert(path.clone(), hash.clone());
            config.prepared_model = Some(model);
            (
                "learned",
                json!({"path": path, "sha256": hash,
                "format_version": metadata.format_version, "architecture_id": metadata.architecture_id,
                "value_divisor": metadata.value_scale, "policy_divisor": metadata.policy_scale,
                "score_scale": score_scale}),
            )
        }
        EvaluatorConfig::External(_) => unreachable!("external handled above"),
    };
    let engine = config.engine;
    let tactical = engine.tactical();
    let (probe, total) = engine.interior_vcf();
    let selection = engine.selectivity();
    let contract = config
        .prepared_model
        .as_ref()
        .map_or(ScoreContract::Pattern, Evaluator::score_contract);
    let profile = engine.effective_profile(contract);
    if engine
        .search_profile()
        .is_some_and(|requested| requested.contract() != contract)
    {
        return Err("selected search profile does not match evaluator score contract".into());
    }
    let model_fingerprint = config.prepared_model.as_ref().map_or_else(
        || match config.evaluator {
            EvaluatorConfig::Pattern => rustmoku_engine::PatternEvaluator.model_fingerprint(),
            _ => None,
        },
        Evaluator::model_fingerprint,
    );
    if engine
        .probcut()
        .is_some_and(|calibration| !calibration.matches(model_fingerprint, profile, selection))
    {
        return Err("ProbCut calibration does not match model/profile/selectivity".into());
    }
    let (vct_probe, vct_total) = engine.interior_vct();
    let book = if let Some(path) = &config.opening_database {
        let path = absolute_file(path)?;
        let mut bytes = Vec::new();
        File::open(&path)?
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        let database = rustmoku_engine::OpeningDatabase::read_from(&mut bytes.as_slice())?;
        let identity = database.identity();
        if identity.engine_build != rustmoku_engine::ENGINE_BUILD_ID
            || identity.model != model_fingerprint
            || identity.profile != profile
        {
            return Err("opening database engine/model/profile mismatch".into());
        }
        let hash = format!("{:x}", Sha256::digest(&bytes));
        inputs.insert(path, hash.clone());
        let description = json!({"kind": "empirical-opening-v1", "sha256": hash,
            "policy": format!("{:?}", config.opening_policy.unwrap_or(rustmoku_engine::OpeningPolicy::OrderOnly)),
            "engine_build": identity.engine_build, "model_fingerprint": identity.model,
            "profile": identity.profile.to_string(), "generation": identity.generation});
        config.prepared_book = Some(std::sync::Arc::new(database));
        description
    } else {
        json!(false)
    };
    Ok(
        json!({"evaluator": evaluator, "model": model, "threads": engine.threads(),
        "tt_mib": engine.tt_memory_mib(), "root_resistance": engine.root_resistance(),
        "adaptive_root_candidates": engine.adaptive_root_candidates(),
        "model_version": config.prepared_model.as_ref().and_then(|model| model.model_format_version()),
        "architecture": config.prepared_model.as_ref().map(|model| model.architecture_name()), "profile": {
            "parameters": profile.to_string(), "score_contract": format!("{:?}", contract),
            "probcut": engine.probcut().map(|calibration| calibration.to_string()),
            "policy_lmr": profile.policy_lmr(), "singular": profile.singular(),
            "interior_vct": {"plies": vct_probe.max_plies, "probe_work": vct_probe.max_nodes,
                "total_work": vct_total, "effective_enabled": engine.threads() == 1 && vct_probe.enabled() && vct_total > 0},
            "vcf": {"plies": tactical.vcf.max_plies, "work": tactical.vcf.max_nodes},
            "vct": {"plies": tactical.vct.max_plies, "work": tactical.vct.max_nodes, "table_mib": tactical.vct_table_memory_mib},
            "interior_vcf": {"plies": probe.max_plies, "probe_work": probe.max_nodes, "total_work": total,
                "effective_enabled": engine.threads() == 1 && probe.enabled() && total > 0},
            "selectivity": {"rfp": selection.reverse_futility, "futility": selection.futility,
                "razor": selection.razoring, "lmp": selection.lmp, "lmr": selection.lmr,
                "iir": selection.iir, "extension": selection.threat_extension}},
        "book": book, "tt_policy": "fresh-per-game-warm-between-moves"}),
    )
}

pub(super) fn describe(options: &mut Options) -> Result<Value, Box<dyn Error>> {
    let mut inputs = BTreeMap::new();
    let executable = input(&std::env::current_exe()?, &mut inputs)?;
    let a = player(&mut options.players[0], &mut inputs)?;
    let b = player(&mut options.players[1], &mut inputs)?;
    let openings = if options.opening_records.is_empty() {
        OPENINGS
            .iter()
            .map(|opening| opening.game().map_err(Into::into))
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?
    } else {
        options
            .opening_records
            .iter()
            .map(|path| {
                let path = input(path, &mut inputs)?;
                if path.metadata()?.len() > 64 * 1024 {
                    return Err("opening record too large".into());
                }
                Ok(Game::from_record(&std::fs::read_to_string(path)?)?)
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?
    };
    if openings
        .iter()
        .any(|game| game.status() != GameStatus::Ongoing)
    {
        return Err("Arena openings must be ongoing legal games".into());
    }
    let openings: Vec<String> = openings
        .iter()
        .map(|game| {
            CanonicalPosition::new(game.position())
                .key()
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        })
        .collect();
    // Pair/leg selection does not alter a player's profile. Every actual leg
    // emits this same description for the manager to compare before counting.
    Ok(json!({"schema": 2, "engine": {"executable": executable,
        "sha256": inputs[&executable], "package_version": env!("CARGO_PKG_VERSION")},
        "players": [a, b], "rules": "15x15-freestyle", "openings": openings,
        "limits": {"depth": options.limits.max_depth, "work": options.limits.max_nodes,
            "turn_hard_ms": options.limits.move_time.map(|time| time.as_millis() as u64),
            "clock_ms": options.clock.map(|time| time.as_millis() as u64),
            "increment_ms": options.increment.as_millis() as u64,
            "time_manager": "completed-stability-pressure-cost-v3-hard90reserve"}, "inputs_sha256": inputs}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empirical_book_freezes_bytes_and_rejects_each_identity_mismatch() {
        let path =
            std::env::temp_dir().join(format!("rustmoku-arena-book-{}.rmopen", std::process::id()));
        let mut config = PlayerConfig {
            opening_database: Some(path.clone()),
            opening_policy: Some(rustmoku_engine::OpeningPolicy::BookMove),
            ..PlayerConfig::default()
        };
        let identity = rustmoku_engine::OpeningIdentity {
            engine_build: rustmoku_engine::ENGINE_BUILD_ID.into(),
            model: rustmoku_engine::PatternEvaluator.model_fingerprint(),
            profile: config.engine.effective_profile(ScoreContract::Pattern),
            generation: "fixture".into(),
        };
        for mismatch in 0..4 {
            let mut candidate = identity.clone();
            match mismatch {
                1 => candidate.engine_build = "wrong-build".into(),
                2 => candidate.model = None,
                3 => candidate.profile = candidate.profile.with_policy_lmr(true),
                _ => (),
            }
            rustmoku_engine::OpeningDatabase::new(candidate)
                .unwrap()
                .write_to_path(&path)
                .unwrap();
            let result = player(&mut config, &mut BTreeMap::new());
            if mismatch == 0 {
                let result = result.unwrap();
                assert_eq!(result["book"]["policy"], "BookMove");
                assert_eq!(result["book"]["sha256"], hash_file(&path).unwrap());
                assert!(config.prepared_book.is_some());
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("engine/model/profile mismatch")
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }
}
