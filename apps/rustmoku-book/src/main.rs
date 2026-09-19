#![forbid(unsafe_code)]
use rustmoku_core::{CanonicalPosition, Game};
use rustmoku_engine::{
    AlphaBetaEngine, CancellationToken, EngineConfig, Evaluator, OpeningDatabase, OpeningEntry,
    OpeningIdentity, OpeningMove, RuntimeEvaluator, SearchLimits,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    path::PathBuf,
};
fn main() {
    if let Err(e) = run() {
        eprintln!("rustmoku-book: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("build|resume|inspect|query|merge")?;
    if command == "build-identity" && args.next().is_none() {
        println!("{}", rustmoku_engine::ENGINE_BUILD_ID);
        return Ok(());
    }
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        let value = args.next().ok_or("missing option value")?;
        if !key.starts_with("--") || options.insert(key, value).is_some() {
            return Err("duplicate/invalid option".into());
        }
    }
    let required = |options: &mut BTreeMap<String, String>, key: &str| {
        options.remove(key).ok_or_else(|| format!("missing {key}"))
    };
    let output = PathBuf::from(required(&mut options, "--output")?);
    match command.as_str() {
        "inspect" => {
            let db = OpeningDatabase::read_from_path(output)?;
            println!(
                "empirical entries={} identity={:?}",
                db.len(),
                db.identity()
            );
        }
        "query" => {
            let game = Game::from_record(&std::fs::read_to_string(required(
                &mut options,
                "--record",
            )?)?)?;
            let db = OpeningDatabase::read_from_path(output)?;
            println!("empirical {:?}", db.query(game.position(), db.identity()));
        }
        "merge" => {
            let mut db = OpeningDatabase::read_from_path(required(&mut options, "--left")?)?;
            db.merge(&OpeningDatabase::read_from_path(required(
                &mut options,
                "--right",
            )?)?)?;
            if !options.is_empty() {
                return Err("unknown merge options".into());
            }
            db.write_to_path(output)?;
        }
        "build" | "resume" => {
            let game = Game::from_record(&std::fs::read_to_string(required(
                &mut options,
                "--record",
            )?)?)?;
            let engine_build = options
                .remove("--engine-build")
                .unwrap_or_else(|| rustmoku_engine::ENGINE_BUILD_ID.to_owned());
            if engine_build != rustmoku_engine::ENGINE_BUILD_ID {
                return Err(
                    "engine-build must match this executable's shared engine identity".into(),
                );
            }
            let max_plies: usize = required(&mut options, "--max-plies")?.parse()?;
            let top_k: usize = required(&mut options, "--top-k")?.parse()?;
            let margin: i32 = required(&mut options, "--score-margin")?.parse()?;
            let depth: u8 = required(&mut options, "--depth")?.parse()?;
            let work: u64 = required(&mut options, "--nodes")?.parse()?;
            let max_positions: usize = required(&mut options, "--max-positions")?.parse()?;
            let max_frontier: usize = options
                .remove("--max-frontier")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(4096);
            if max_plies > 225
                || top_k == 0
                || top_k > 225
                || margin < 0
                || depth == 0
                || work == 0
                || max_positions == 0
                || max_frontier == 0
                || max_frontier > 100_000
            {
                return Err("invalid opening limits".into());
            }
            let evaluator = options
                .remove("--model")
                .map(RuntimeEvaluator::read_from_path)
                .transpose()?
                .unwrap_or(RuntimeEvaluator::Pattern);
            let profile = options.remove("--profile").map(|p| p.parse()).transpose()?;
            let mut config = EngineConfig::new(16);
            if let Some(profile) = profile {
                config = config.with_search_profile(profile);
            }
            if !options.is_empty() {
                return Err("unknown opening build options".into());
            }
            let identity = OpeningIdentity {
                engine_build,
                model: evaluator.model_fingerprint(),
                profile: config.effective_profile(evaluator.score_contract()),
                generation: format!(
                    "beam-v1;plies={max_plies};k={top_k};margin={margin};depth={depth};work={work};root={:?}",
                    CanonicalPosition::new(game.position()).key()
                ),
            };
            let mut db = if command == "resume" {
                let db = OpeningDatabase::read_from_path(&output)?;
                if db.identity() != &identity {
                    return Err("incompatible opening resume".into());
                }
                db
            } else {
                if output.exists() {
                    return Err("output exists; use resume".into());
                }
                OpeningDatabase::new(identity.clone())?
            };
            let engine = AlphaBetaEngine::with_config(evaluator, config);
            let root_ply = game.position().move_count();
            let mut queue = VecDeque::from([game]);
            let mut seen = BTreeSet::new();
            let mut generated = 0;
            let mut resource_stop = false;
            'build: while let Some(game) = queue.pop_front() {
                // Offline generation uses one orientation, including after resume.
                // Search itself keeps its ordinary noncanonical hot-path identity.
                let canonical = CanonicalPosition::new(game.position());
                let mut normalized = Game::new(game.position().rules());
                for at in game.history() {
                    normalized.play_move(canonical.move_to_canonical(at))?;
                }
                let game = normalized;
                if seen.len() >= 100_000 {
                    resource_stop = true;
                    break;
                }
                if game.position().winner().is_some()
                    || game.position().is_full()
                    || game.position().move_count() - root_ply >= max_plies
                {
                    continue;
                }
                if !seen.insert(CanonicalPosition::new(game.position()).key()) {
                    continue;
                }
                let entry = if let Some(entry) = db.query(game.position(), &identity) {
                    entry
                } else {
                    if generated >= max_positions {
                        break;
                    }
                    let analysis = engine.analyze_root(
                        game.position(),
                        SearchLimits::new(depth).with_max_nodes(work),
                        16,
                        CancellationToken::new(),
                    )?;
                    if analysis.completed_depth == 0 {
                        continue;
                    }
                    let mut moves: Vec<_> = analysis
                        .candidates
                        .iter()
                        .filter_map(|c| {
                            c.score.map(|score| OpeningMove {
                                at: c.at,
                                score: score.clamp(-10_000_000, 10_000_000),
                            })
                        })
                        .collect();
                    moves.sort_by_key(|m| (std::cmp::Reverse(m.score), m.at.index()));
                    if moves.is_empty() {
                        continue;
                    }
                    let entry = OpeningEntry {
                        moves,
                        depth: analysis.completed_depth,
                        work: analysis.work,
                        domain: "all-legal/production-radius-two/four-q6-immediate-v1".into(),
                        wdl: None,
                    };
                    db.insert(game.position(), entry.clone())?;
                    generated += 1;
                    db.write_to_path(&output)?;
                    db.query(game.position(), &identity)
                        .ok_or("published opening entry missing")?
                };
                let best = entry.moves[0].score;
                let retain = if entry
                    .moves
                    .get(1)
                    .is_some_and(|m| i64::from(best) - i64::from(m.score) <= i64::from(margin))
                {
                    top_k
                } else {
                    1
                };
                let selected: BTreeSet<_> = entry
                    .moves
                    .iter()
                    .take(retain)
                    .map(|m| m.at.index())
                    .chain(
                        OpeningDatabase::protected_moves(game.position())
                            .into_iter()
                            .map(|m| m.index()),
                    )
                    .collect();
                for index in selected {
                    if queue.len() >= max_frontier {
                        resource_stop = true;
                        break 'build;
                    }
                    let mut child = Game::from_record(&game.to_record())?;
                    child.play_move(rustmoku_core::Move::from_index(index)?)?;
                    queue.push_back(child);
                }
            }
            db.write_to_path(output)?;
            println!(
                "empirical entries={} newly_generated={generated} frontier_limit={resource_stop}",
                db.len()
            );
        }
        _ => return Err("unknown opening command".into()),
    }
    if !options.is_empty() {
        return Err("unknown options".into());
    }
    Ok(())
}
