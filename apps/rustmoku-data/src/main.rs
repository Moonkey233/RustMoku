#![forbid(unsafe_code)]

mod pipe;

use std::{
    env,
    error::Error,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
};

use rustmoku_core::{CanonicalPosition, CanonicalPositionKey, Game, Move, OPENINGS, Symmetry};
use rustmoku_engine::{
    AlphaBetaEngine, EngineConfig, Evaluator, ProofBook, ProofBookVerifyLimits, ProofDistance,
    RuntimeEvaluator, SearchEngine, SearchLimits, SearchOrigin, SearchTermination,
};

const MAGIC: &[u8; 8] = b"RMDATA01";
const VERSION: u16 = 2;
const MAX_RECORDS: usize = 10_000_000;
const LEGACY_RECORD_BYTES: usize = 8 + 2 + 1 + 1 + 4 + 1 + 1 + CanonicalPositionKey::BYTE_LEN;

const QUALITY_BYTES: usize = 19;
const RECORD_BYTES: usize = LEGACY_RECORD_BYTES + QUALITY_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DataRecord {
    game_id: u64,
    ply: u16,
    canonical_symmetry: u8,
    policy_move: Option<Move>,
    value: i32,
    source: SearchOrigin,
    exact: bool,
    position: CanonicalPositionKey,
    quality: Option<LabelQuality>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LabelQuality {
    completed_depth: u8,
    requested_depth: u8,
    termination: u8,
    work: u64,
    budget: u64,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rustmoku-data: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = Arguments::new(env::args().skip(1));
    match args.command()?.as_str() {
        "pipe" => pipe_player(args),
        "record" => generate_record(args),
        "analyze" => analyze_record(args),
        "proof" => generate_proof(args),
        "opening" => generate_opening(args),
        "selfplay" => generate_selfplay(args),
        "inspect" => inspect(args),
        "model-check" => model_check(args),
        "backend-info" => {
            args.finish()?;
            println!("architecture={}", std::env::consts::ARCH);
            println!(
                "auto={}",
                rustmoku_engine::EvaluatorBackend::detect().name()
            );
            println!(
                "avx2={}",
                rustmoku_engine::EvaluatorBackend::avx2().is_some()
            );
            Ok(())
        }
        "help" | "--help" | "-h" => {
            usage();
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
}

fn generate_opening(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let database =
        rustmoku_engine::OpeningDatabase::read_from_path(args.required("--opening-db")?)?;
    let output = PathBuf::from(args.required("--output")?);
    let maximum: usize = args.optional("--max-positions")?.unwrap_or(1000);
    args.finish()?;
    if maximum == 0 || maximum > 100_000 {
        return Err("opening export limit must be 1..100000".into());
    }
    let mut records = Vec::new();
    for (index, key) in database
        .start_keys(0, 225)
        .into_iter()
        .take(maximum)
        .enumerate()
    {
        let game = database.replay_start(key)?;
        let entry = database
            .query(game.position(), database.identity())
            .ok_or("opening query failed")?;
        let best = entry.moves.first().ok_or("opening has no ranked move")?;
        records.push(DataRecord {
            game_id: index as u64,
            ply: game.position().move_count() as u16,
            canonical_symmetry: 0,
            policy_move: Some(best.at),
            value: best.score,
            source: SearchOrigin::OpeningBook,
            exact: false,
            position: key,
            quality: Some(LabelQuality {
                completed_depth: entry.depth,
                requested_depth: entry.depth,
                termination: 0,
                work: entry.work,
                budget: entry.work,
            }),
        });
    }
    write_dataset(&output, &records)?;
    println!("empirical opening records={}", records.len());
    Ok(())
}

fn pipe_player(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let model: Option<PathBuf> = args.optional("--model")?;
    let profile: Option<rustmoku_engine::SearchProfile> = args
        .optional::<String>("--profile")?
        .map(|value| value.parse())
        .transpose()?;
    let describe = args.optional("--describe")?.unwrap_or(false);
    let depth = args.optional("--depth")?.unwrap_or(64);
    let nodes = args.optional("--nodes")?.unwrap_or(u64::MAX);
    let threads: usize = args.optional("--threads")?.unwrap_or(1);
    let tt_mib: usize = args.optional("--tt-mib")?.unwrap_or(64);
    let root_resistance = args.optional("--root-resistance")?.unwrap_or(true);
    let adaptive_root_candidates = args
        .optional("--adaptive-root-candidates")?
        .unwrap_or(false);
    args.finish()?;
    if !(1..=8).contains(&threads) || tt_mib > 1024 || depth == 0 || nodes == 0 {
        return Err("invalid pipe resource limits".into());
    }
    let evaluator = model
        .map(RuntimeEvaluator::read_from_path)
        .transpose()?
        .unwrap_or(RuntimeEvaluator::Pattern);
    let config = teacher_config(&evaluator, profile)?
        .with_threads(threads)
        .with_root_resistance(root_resistance)
        .with_adaptive_root_candidates(adaptive_root_candidates)
        .with_tt_memory_mib(tt_mib);
    if describe {
        let fingerprint = evaluator
            .model_fingerprint()
            .ok_or("missing model identity")?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        println!(
            r#"{{"protocol":"rustmoku-pipe-v1","version":"{}","model_fingerprint":"{}","profile":"{}","threads":{},"tt_mib":{},"depth":{},"nodes":{},"root_resistance":{},"evaluator":"{}","model_version":{},"score_contract":"{:?}","adaptive_root_candidates":{}}}"#,
            env!("CARGO_PKG_VERSION"),
            fingerprint,
            config.effective_profile(evaluator.score_contract()),
            threads,
            tt_mib,
            depth,
            nodes,
            root_resistance,
            evaluator.architecture_name(),
            evaluator
                .model_format_version()
                .map_or_else(|| "null".to_owned(), |v| v.to_string()),
            evaluator.score_contract(),
            adaptive_root_candidates
        );
        return Ok(());
    }
    pipe::run(
        AlphaBetaEngine::with_config(evaluator, config),
        depth,
        nodes,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}

/// Explicit offline analysis uses a shared work cap and a common completed horizon.
fn analyze_record(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let input = PathBuf::from(args.required("--record")?);
    let depth = args.optional("--depth")?.unwrap_or(4);
    let nodes = args.optional("--nodes")?.unwrap_or(20_000);
    let top_k = args.optional("--top-k")?.unwrap_or(8);
    let score_only = args.optional("--score-only")?.unwrap_or(false);
    let universe = match args
        .optional::<String>("--candidates")?
        .as_deref()
        .unwrap_or("practical")
    {
        "practical" => rustmoku_engine::TeacherCandidates::Practical,
        "all-legal" | "reference-oracle" => rustmoku_engine::TeacherCandidates::AllLegal,
        "production-top-k" => rustmoku_engine::TeacherCandidates::ProductionTopK,
        _ => {
            return Err(
                "candidates must be practical, reference-oracle, all-legal or production-top-k"
                    .into(),
            );
        }
    };
    let model: Option<PathBuf> = args.optional("--model")?;
    let profile: Option<rustmoku_engine::SearchProfile> = args
        .optional::<String>("--profile")?
        .map(|value| value.parse())
        .transpose()?;
    args.finish()?;
    let evaluator = model
        .map(RuntimeEvaluator::read_from_path)
        .transpose()?
        .unwrap_or(RuntimeEvaluator::Pattern);
    let game = Game::from_record(&std::fs::read_to_string(input)?)?;
    let engine =
        AlphaBetaEngine::with_config(evaluator.clone(), teacher_config(&evaluator, profile)?);
    if score_only {
        let result = engine.analyze_score(
            game.position(),
            SearchLimits::new(depth).with_max_nodes(nodes),
            rustmoku_engine::CancellationToken::new(),
        )?;
        let score = result
            .score
            .map_or_else(|| "null".to_string(), |value| value.to_string());
        let key = CanonicalPosition::new(game.position())
            .key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        println!(
            r#"{{"version":1,"position_key":"{}","stones":{},"perspective":"root-side-to-move","score":{},"requested_depth":{},"completed_depth":{},"quiet":{},"termination":"{:?}","work":{}}}"#,
            key,
            game.position().move_count(),
            score,
            depth,
            result.completed_depth,
            result.quiet,
            result.termination,
            result.work
        );
        return Ok(());
    }
    let result = engine.analyze_root_with_candidates(
        game.position(),
        SearchLimits::new(depth).with_max_nodes(nodes),
        top_k,
        universe,
        rustmoku_engine::CancellationToken::new(),
    )?;
    println!("{}", analysis_json(&game, &result, depth, nodes, top_k));
    Ok(())
}

fn analysis_json(
    game: &Game,
    result: &rustmoku_engine::RootAnalysis,
    depth: u8,
    nodes: u64,
    top_k: usize,
) -> String {
    let canonical = CanonicalPosition::new(game.position());
    let key = canonical
        .key()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let production = rustmoku_engine::ProductionCandidateUniverse::new(game.position());
    let recall = if result.universe != rustmoku_engine::TeacherCandidates::ProductionTopK
        && result.completed_depth > 0
    {
        let mut ranked: Vec<_> = result.candidates.iter().collect();
        ranked.sort_by_key(|candidate| (std::cmp::Reverse(candidate.score), candidate.at));
        let count = top_k.min(ranked.len());
        let hits = ranked
            .iter()
            .take(count)
            .filter(|candidate| production.contains(candidate.at))
            .count();
        let canonical_best = ranked
            .first()
            .is_some_and(|candidate| production.contains(candidate.at));
        // A tied low-index teacher move outside the radius is not a value miss
        // when an equally good production move exists. Report both questions.
        let best = ranked.first().is_some_and(|first| {
            ranked.iter().any(|candidate| {
                candidate.score == first.score && production.contains(candidate.at)
            })
        });
        format!(
            r#"{{"best_in_production":{best},"canonical_best_in_production":{canonical_best},"top_k":{count},"top_k_hits":{hits},"top_k_tie_break":"move-index"}}"#
        )
    } else {
        "null".to_owned()
    };
    let candidates = result.candidates.iter().map(|candidate| {
        let score = candidate.score.map_or_else(|| "null".to_string(), |value| value.to_string());
        format!(r#"{{"move":{},"score":{},"bound":"{:?}","completed_depth":{},"nominal_depth_valid":{},"source":"{:?}","termination":"{:?}","work":{},"in_production":{}}}"#,
            canonical.move_to_canonical(candidate.at).index(), score, candidate.bound, candidate.completed_depth,
            candidate.nominal_depth_valid, candidate.source, candidate.termination, candidate.work, production.contains(candidate.at))
    }).collect::<Vec<_>>().join(",");
    format!(
        r#"{{"version":4,"position_key":"{}","perspective":"root-side-to-move","requested_depth":{},"completed_depth":{},"termination":"{:?}","work":{},"budget":{},"candidates":[{}],"candidate_universe":"{:?}","root_universe":"{}","descendant_universe":"{}","leaf_policy":"four-q6-immediate-v1","search_domain":"{}","selectivity":"candidate-domain-only-no-depth-pruning","score_reference_scale":{},"production_recall":{}}}"#,
        key,
        depth,
        result.completed_depth,
        result.termination,
        result.work,
        nodes,
        candidates,
        result.universe,
        result.universe.root_universe(),
        result.universe.descendant_universe(),
        result.universe.search_domain(),
        result.score_contract.reference_scale(),
        recall
    )
}

fn generate_proof(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let book = PathBuf::from(args.required("--book")?);
    let output = PathBuf::from(args.required("--output")?);
    let max_positions: usize = args.optional("--max-positions")?.unwrap_or(10_000);
    let max_work: u64 = args.optional("--verify-work")?.unwrap_or(1_000_000);
    args.finish()?;
    if max_positions == 0 || max_positions > 100_000 {
        return Err("proof export positions must be 1..=100000".into());
    }
    let verified = ProofBook::read_from_path(book)?
        .verify_with_limits(ProofBookVerifyLimits::default().with_total_work(max_work))?;
    let mut records = Vec::new();
    let mut metadata = Vec::new();
    verified.visit_training_positions(max_positions, |sample| {
        let canonical = CanonicalPosition::new(sample.game.position());
        let game_id = records.len() as u64;
        records.push(DataRecord {
            game_id, ply: sample.game.history().len() as u16,
            canonical_symmetry: symmetry_tag(canonical.original_to_canonical()),
            policy_move: sample.policy.map(|at| canonical.move_to_canonical(at)),
            value: i32::from(sample.value) * 10_000_000,
            source: SearchOrigin::ProofBook, exact: true, position: canonical.key(), quality: None,
        });
        let lineage = sample.lineage.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let moves = sample.game.history().map(|at| at.index().to_string()).collect::<Vec<_>>().join(",");
        let kind = match sample.distance { ProofDistance::Exact(_) => "source-exact", ProofDistance::AtMost(_) => "at-most" };
        metadata.push(format!(
            "\"{game_id}\":{{\"lineage_id\":\"{lineage}-{:?}\",\"distance_kind\":\"{kind}\",\"plies\":{},\"moves\":[{moves}]}}",
            sample.attacker, sample.distance.plies()));
    })?;
    write_dataset(&output, &records)?;
    write_companion(
        &output.with_extension("proof.json"),
        format!("{{\"version\":1,\"games\":{{{}}}}}\n", metadata.join(",")).as_bytes(),
    )?;
    println!(
        "exported {} independently verified proof labels",
        records.len()
    );
    Ok(())
}

fn generate_record(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let input = PathBuf::from(args.required("--record")?);
    let output = PathBuf::from(args.required("--output")?);
    let depth = args.optional("--depth")?.unwrap_or(8);
    let nodes = args.optional("--nodes")?.unwrap_or(50_000);
    let branch_ply: Option<usize> = args.optional("--branch-ply")?;
    let branch_choice: usize = args.optional("--branch-choice")?.unwrap_or(0);
    let model: Option<PathBuf> = args.optional("--model")?;
    args.finish()?;
    let source = Game::from_record(&std::fs::read_to_string(input)?)?;
    let source = if let Some(ply) = branch_ply {
        if ply >= source.history().len() {
            return Err("branch ply must precede an existing historical move".into());
        }
        let mut branch = Game::new(source.position().rules());
        for at in source.history().take(ply) {
            branch.play_move(at)?;
        }
        let original = source.history().nth(ply).unwrap();
        let alternatives: Vec<_> = Move::all()
            .filter(|&at| at != original && branch.position().is_legal(at))
            .collect();
        if alternatives.is_empty() {
            return Err("branch has no legal alternative".into());
        }
        branch.play_move(alternatives[branch_choice % alternatives.len()])?;
        branch
    } else {
        source
    };
    let mut trajectory = Game::new(source.position().rules());
    let evaluator = model
        .map(RuntimeEvaluator::read_from_path)
        .transpose()?
        .unwrap_or(RuntimeEvaluator::Pattern);
    let mut teacher =
        AlphaBetaEngine::with_config(evaluator, EngineConfig::new(64).with_threads(1));
    let mut records = Vec::with_capacity(source.history().len() + 1);
    for (ply, historical) in source.history().enumerate() {
        records.push(label_position(
            &mut teacher,
            0,
            ply,
            &trajectory,
            depth,
            nodes,
        )?);
        trajectory.play_move(historical)?;
    }
    records.push(label_position(
        &mut teacher,
        0,
        trajectory.history().len(),
        &trajectory,
        depth,
        nodes,
    )?);
    write_dataset(&output, &records)?;
    println!("wrote {} records to {}", records.len(), output.display());
    Ok(())
}

type RootComparison = (u64, usize, String);
type SelfplayBatch = (Vec<DataRecord>, Vec<RootComparison>);

fn generate_selfplay(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let output = PathBuf::from(args.required("--output")?);
    let games: usize = args.required("--games")?.parse()?;
    let seed: u64 = args.required("--seed")?.parse()?;
    let requested_workers: usize = args.optional("--workers")?.unwrap_or(1);
    let depth = args.optional("--depth")?.unwrap_or(6);
    let nodes = args.optional("--nodes")?.unwrap_or(20_000);
    let random_plies: usize = args.optional("--random-plies")?.unwrap_or(0);
    let opening_database = args
        .optional::<PathBuf>("--opening-db")?
        .map(rustmoku_engine::OpeningDatabase::read_from_path)
        .transpose()?;
    let opening_keys = opening_database
        .as_ref()
        .map(|db| db.start_keys(2, 16))
        .unwrap_or_default();
    if opening_database.is_some() && opening_keys.is_empty() {
        return Err("opening database has no eligible 2..16-ply starts".into());
    }
    let explore_top_k: usize = args.optional("--explore-top-k")?.unwrap_or(0);
    let explore_temperature: f64 = args.optional("--explore-temperature")?.unwrap_or(1000.0);
    let explore_plies: usize = args.optional("--explore-plies")?.unwrap_or(80);
    let cold_games: bool = args.optional("--cold-games")?.unwrap_or(true);
    let first_game: u64 = args.optional("--first-game")?.unwrap_or(0);
    let model: Option<PathBuf> = args.optional("--model")?;
    let profile: Option<rustmoku_engine::SearchProfile> = args
        .optional::<String>("--profile")?
        .map(|value| value.parse())
        .transpose()?;
    let evaluator = model
        .map(RuntimeEvaluator::read_from_path)
        .transpose()?
        .unwrap_or(RuntimeEvaluator::Pattern);
    args.finish()?;
    if random_plies > 8 {
        return Err("random prefix must be 0..=8 plies".into());
    }
    if explore_top_k > 16
        || !explore_temperature.is_finite()
        || explore_temperature <= 0.0
        || explore_plies > 225
    {
        return Err("invalid bounded teacher exploration configuration".into());
    }
    if games == 0 || games > 32 {
        return Err("selfplay accepts 1..=32 games per shard".into());
    }
    if requested_workers == 0 || requested_workers > 4 {
        return Err("workers must be in 1..=4".into());
    }
    if !cold_games {
        return Err("selfplay requires cold-per-game TT for worker-independent semantics".into());
    }
    first_game
        .checked_add(games as u64)
        .ok_or("stable game id overflow")?;
    let config = teacher_config(&evaluator, profile)?;
    let workers = requested_workers.min(games);
    let (mut records, mut comparisons) = thread::scope(|scope| {
        let mut handles = Vec::new();
        for worker in 0..workers {
            let evaluator = evaluator.clone();
            let opening_database = &opening_database;
            let opening_keys = &opening_keys;
            handles.push(scope.spawn(move || -> Result<SelfplayBatch, String> {
                // Independent persistent teacher state: no globally locked
                // engine and no scheduler-dependent cross-worker TT sharing.
                let mut teacher = AlphaBetaEngine::with_config(evaluator, config);
                let mut records = Vec::new();
                let mut comparisons = Vec::new();
                for local_game in (worker..games).step_by(workers) {
                    let game_id = first_game + local_game as u64;
                    teacher.clear_transposition_table();
                    let choice = splitmix64(seed ^ game_id) as usize;
                    let mut game = if let Some(db) = opening_database {
                        db.replay_start(opening_keys[choice % opening_keys.len()])
                            .map_err(|error| error.to_string())?
                    } else {
                        OPENINGS[choice % OPENINGS.len()]
                            .game()
                            .map_err(|error| error.to_string())?
                    };
                    let opening_plies = game.history().len();
                    while game.status() == rustmoku_core::GameStatus::Ongoing {
                        let record = label_position(
                            &mut teacher,
                            game_id,
                            game.history().len(),
                            &game,
                            depth,
                            nodes,
                        )
                        .map_err(|error| error.to_string())?;
                        let Some(at) = record.policy_move.map(|at| {
                            let canonical = CanonicalPosition::new(game.position());
                            canonical.move_to_original(at)
                        }) else {
                            break;
                        };
                        let seed =
                            splitmix64(seed ^ splitmix64(game_id) ^ game.history().len() as u64);
                        let explored = if explore_top_k > 0
                            && game.history().len() < explore_plies
                            && !record.exact
                        {
                            let analysis = teacher
                                .analyze_root(
                                    game.position(),
                                    SearchLimits::new(depth).with_max_nodes(nodes),
                                    explore_top_k,
                                    rustmoku_engine::CancellationToken::new(),
                                )
                                .map_err(str::to_string)?;
                            comparisons.push((
                                game_id,
                                game.history().len(),
                                analysis_json(&game, &analysis, depth, nodes, explore_top_k),
                            ));
                            near_optimal_move(
                                &game,
                                &analysis,
                                at,
                                seed,
                                explore_temperature,
                                explore_top_k,
                            )
                        } else {
                            at
                        };
                        let at = if game.history().len() - opening_plies < random_plies {
                            diverse_move(&game, at, seed)
                        } else {
                            explored
                        };
                        records.push(record);
                        game.play_move(at).map_err(|error| error.to_string())?;
                    }
                    records.push(
                        label_position(
                            &mut teacher,
                            game_id,
                            game.history().len(),
                            &game,
                            depth,
                            nodes,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
                Ok((records, comparisons))
            }));
        }
        let mut records = Vec::new();
        let mut comparisons = Vec::new();
        for handle in handles {
            let (rows, labels) = handle.join().map_err(|_| "data worker panicked")??;
            records.extend(rows);
            comparisons.extend(labels);
        }
        Ok::<_, String>((records, comparisons))
    })?;
    records.sort_by_key(|record| (record.game_id, record.ply));
    if explore_top_k > 0 {
        comparisons.sort_by_key(|(game, ply, _)| (*game, *ply));
        let content = comparisons
            .into_iter()
            .map(|(_, _, value)| value + "\n")
            .collect::<String>();
        write_companion(&output.with_extension("policy.jsonl"), content.as_bytes())?;
    }
    if records.len() > MAX_RECORDS {
        return Err("generated record count exceeds dataset safety limit".into());
    }
    write_dataset(&output, &records)?;
    println!(
        "wrote {} deterministic records from {games} games to {}",
        records.len(),
        output.display()
    );
    Ok(())
}

fn near_optimal_move(
    game: &Game,
    analysis: &rustmoku_engine::RootAnalysis,
    fallback: Move,
    seed: u64,
    temperature: f64,
    top_k: usize,
) -> Move {
    let position = game.position();
    if Move::all().any(|at| {
        position.is_legal(at)
            && (position.would_win(at, position.side_to_move())
                || position.would_win(at, position.side_to_move().opponent()))
    }) {
        return fallback;
    }
    let mut candidates: Vec<_> = analysis
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.bound == rustmoku_engine::CandidateBound::DomainExact
                && candidate.nominal_depth_valid
                && candidate.completed_depth == analysis.completed_depth
                && candidate.completed_depth > 0
                && candidate
                    .score
                    .is_some_and(|score| score.abs() <= 10_000_000)
        })
        .collect();
    candidates.sort_by_key(|candidate| (std::cmp::Reverse(candidate.score), candidate.at));
    candidates.truncate(top_k);
    let Some(maximum) = candidates
        .iter()
        .filter_map(|candidate| candidate.score)
        .max()
    else {
        return fallback;
    };
    let temperature = temperature * f64::from(analysis.score_contract.reference_scale()) / 10_000.0;
    let weights: Vec<_> = candidates
        .iter()
        .map(|candidate| {
            ((f64::from(candidate.score.unwrap()) - f64::from(maximum)) / temperature).exp()
        })
        .collect();
    let total: f64 = weights.iter().sum();
    let mut draw = ((seed >> 11) as f64 / (1_u64 << 53) as f64) * total;
    for (candidate, weight) in candidates.iter().zip(weights) {
        draw -= weight;
        if draw < 0.0 {
            return candidate.at;
        }
    }
    candidates.last().map_or(fallback, |candidate| candidate.at)
}

fn write_companion(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let temporary = path.with_extension(format!("{}.partial", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if path.exists() {
            if !files_equal(&temporary, path)? {
                return Err("immutable companion changed".into());
            }
        } else {
            std::fs::hard_link(&temporary, path)?;
        }
        Ok(())
    })();
    std::fs::remove_file(temporary)?;
    result
}

// Offline prefix diversity never changes normal engine determinism or labels.
// Exact wins/blocks on either side suppress exploration entirely.
fn diverse_move(game: &Game, teacher: Move, seed: u64) -> Move {
    let position = game.position();
    if Move::all().any(|at| {
        position.is_legal(at)
            && (position.would_win(at, position.side_to_move())
                || position.would_win(at, position.side_to_move().opponent()))
    }) {
        return teacher;
    }
    let legal: Vec<_> = Move::all().filter(|&at| position.is_legal(at)).collect();
    legal[(seed % legal.len() as u64) as usize]
}

fn label_position<E: Evaluator>(
    engine: &mut AlphaBetaEngine<E>,
    game_id: u64,
    ply: usize,
    game: &Game,
    depth: u8,
    nodes: u64,
) -> Result<DataRecord, Box<dyn Error>> {
    let result = engine.search(
        game.position(),
        SearchLimits::new(depth).with_max_nodes(nodes),
    );
    let canonical = CanonicalPosition::new(game.position());
    Ok(DataRecord {
        game_id,
        ply: u16::try_from(ply)?,
        canonical_symmetry: symmetry_tag(canonical.original_to_canonical()),
        policy_move: result.best_move.map(|at| canonical.move_to_canonical(at)),
        value: result.score,
        source: result.origin,
        exact: matches!(
            result.origin,
            SearchOrigin::Terminal
                | SearchOrigin::Immediate
                | SearchOrigin::Vcf
                | SearchOrigin::Vct
                | SearchOrigin::ProofBook
        ),
        position: canonical.key(),
        quality: Some(LabelQuality {
            completed_depth: result.completed_depth,
            requested_depth: depth,
            termination: match result.termination {
                SearchTermination::Completed => 0,
                SearchTermination::NodeLimit => 1,
                SearchTermination::TimeLimit => 2,
                SearchTermination::Cancelled => 3,
            },
            work: result.statistics.work_nodes,
            budget: nodes,
        }),
    })
}

fn teacher_config(
    evaluator: &RuntimeEvaluator,
    profile: Option<rustmoku_engine::SearchProfile>,
) -> Result<EngineConfig, Box<dyn Error>> {
    let config = EngineConfig::new(64).with_threads(1);
    if let Some(profile) = profile {
        if profile.contract() != evaluator.score_contract() {
            return Err("teacher profile score contract mismatch".into());
        }
        Ok(config.with_search_profile(profile))
    } else {
        Ok(config)
    }
}

fn write_dataset(path: &Path, records: &[DataRecord]) -> Result<(), Box<dyn Error>> {
    let temporary = path.with_extension(format!("{}.partial", std::process::id()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        write_dataset_file(file, records)?;
        if path.exists() {
            if !files_equal(&temporary, path)? {
                return Err("immutable dataset output changed".into());
            }
        } else {
            std::fs::hard_link(&temporary, path)?;
        }
        Ok(())
    })();
    if temporary.exists() {
        std::fs::remove_file(&temporary)?;
    }
    result
}

fn files_equal(a: &Path, b: &Path) -> std::io::Result<bool> {
    if std::fs::symlink_metadata(b)?.file_type().is_symlink() {
        return Ok(false);
    }
    let (mut a, mut b) = (File::open(a)?, File::open(b)?);
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let (mut left, mut right) = ([0; 8192], [0; 8192]);
    loop {
        let count = a.read(&mut left)?;
        if count == 0 {
            return Ok(true);
        }
        b.read_exact(&mut right[..count])?;
        if left[..count] != right[..count] {
            return Ok(false);
        }
    }
}

fn write_dataset_file(mut file: File, records: &[DataRecord]) -> Result<(), Box<dyn Error>> {
    if records.len() > MAX_RECORDS {
        return Err("dataset record count exceeds safety limit".into());
    }
    file.write_all(MAGIC)?;
    file.write_all(&VERSION.to_le_bytes())?;
    file.write_all(&0_u16.to_le_bytes())?;
    file.write_all(&u32::try_from(records.len())?.to_le_bytes())?;
    for record in records {
        file.write_all(&record.game_id.to_le_bytes())?;
        file.write_all(&record.ply.to_le_bytes())?;
        file.write_all(&[record.canonical_symmetry])?;
        file.write_all(&[record
            .policy_move
            .map_or(u8::MAX, |at| u8::try_from(at.index()).unwrap())])?;
        file.write_all(&record.value.to_le_bytes())?;
        file.write_all(&[origin_tag(record.source), u8::from(record.exact)])?;
        file.write_all(record.position.as_bytes())?;
        if let Some(quality) = record.quality {
            file.write_all(&[
                quality.completed_depth,
                quality.requested_depth,
                quality.termination,
            ])?;
            file.write_all(&quality.work.to_le_bytes())?;
            file.write_all(&quality.budget.to_le_bytes())?;
        } else {
            file.write_all(&[u8::MAX; QUALITY_BYTES])?;
        }
    }
    file.sync_all()?;
    Ok(())
}

fn read_dataset(path: &Path) -> Result<Vec<DataRecord>, Box<dyn Error>> {
    let length = std::fs::metadata(path)?.len();
    let maximum = 16_u64 + MAX_RECORDS as u64 * RECORD_BYTES as u64;
    if length > maximum {
        return Err("dataset file exceeds safety limit".into());
    }
    let mut file = File::open(path)?;
    let mut header = [0; 16];
    file.read_exact(&mut header)?;
    let version = u16::from_le_bytes(header[8..10].try_into().unwrap());
    if &header[..8] != MAGIC
        || !matches!(version, 1 | VERSION)
        || u16::from_le_bytes(header[10..12].try_into().unwrap()) != 0
    {
        return Err("invalid dataset magic, version, or flags".into());
    }
    let count = usize::try_from(u32::from_le_bytes(header[12..16].try_into().unwrap()))?;
    let record_bytes = if version == 1 {
        LEGACY_RECORD_BYTES
    } else {
        RECORD_BYTES
    };
    if count > MAX_RECORDS || length != 16 + count as u64 * record_bytes as u64 {
        return Err("invalid dataset record count or length".into());
    }
    let mut records = Vec::with_capacity(count);
    let mut bytes = vec![0; record_bytes];
    for _ in 0..count {
        file.read_exact(&mut bytes)?;
        let symmetry = bytes[10];
        if symmetry >= 8 {
            return Err("invalid canonical symmetry tag".into());
        }
        let policy_move = match bytes[11] {
            u8::MAX => None,
            value => Some(Move::from_index(usize::from(value))?),
        };
        let source = decode_origin(bytes[16])?;
        let exact = match bytes[17] {
            0 => false,
            1 => true,
            _ => return Err("invalid exact-label flag".into()),
        };
        let key: [u8; CanonicalPositionKey::BYTE_LEN] =
            bytes[18..LEGACY_RECORD_BYTES].try_into()?;
        records.push(DataRecord {
            game_id: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            ply: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
            canonical_symmetry: symmetry,
            policy_move,
            value: i32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            source,
            exact,
            position: CanonicalPositionKey::from_bytes(key)?,
            quality: if version == 1 || bytes[LEGACY_RECORD_BYTES..].iter().all(|&b| b == u8::MAX) {
                None
            } else {
                let q = &bytes[LEGACY_RECORD_BYTES..];
                let quality = LabelQuality {
                    completed_depth: q[0],
                    requested_depth: q[1],
                    termination: q[2],
                    work: u64::from_le_bytes(q[3..11].try_into()?),
                    budget: u64::from_le_bytes(q[11..19].try_into()?),
                };
                if quality.termination > 3 || quality.work > quality.budget {
                    return Err("invalid label quality metadata".into());
                }
                Some(quality)
            },
        });
    }
    Ok(records)
}

fn inspect(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(args.required("--dataset")?);
    args.finish()?;
    let records = read_dataset(&path)?;
    let games = records
        .iter()
        .map(|record| record.game_id)
        .max()
        .map_or(0, |last| last + 1);
    let exact = records.iter().filter(|record| record.exact).count();
    println!(
        "version={VERSION} records={} games={games} exact={exact} bytes={}",
        records.len(),
        std::fs::metadata(path)?.len()
    );
    Ok(())
}

fn model_check(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let model = PathBuf::from(args.required("--model")?);
    let record = PathBuf::from(args.required("--record")?);
    let at: Option<Move> = args.take("--move")?.map(|text| text.parse()).transpose()?;
    let backend = args.take("--backend")?.unwrap_or_else(|| "auto".into());
    args.finish()?;
    let game = Game::from_record(&std::fs::read_to_string(record)?)?;
    let evaluator = RuntimeEvaluator::read_from_path(model)?;
    let selected = match backend.as_str() {
        "auto" => rustmoku_engine::EvaluatorBackend::detect(),
        "scalar" => rustmoku_engine::EvaluatorBackend::SCALAR,
        "avx2" => rustmoku_engine::EvaluatorBackend::avx2().ok_or("AVX2 unavailable")?,
        _ => return Err("backend must be auto, scalar or avx2".into()),
    };
    let evaluator = match evaluator {
        RuntimeEvaluator::MixLite(evaluator) => {
            RuntimeEvaluator::MixLite(evaluator.with_backend(selected))
        }
        other if backend != "avx2" => other,
        _ => return Err("explicit AVX2 backend requires MixLite V3".into()),
    };
    println!("value={}", evaluator.evaluate_position(game.position()));
    if let Some(at) = at {
        println!(
            "policy={}",
            evaluator
                .policy_for(game.position(), at)
                .ok_or("policy move is illegal")?
        );
    }
    Ok(())
}

const fn origin_tag(origin: SearchOrigin) -> u8 {
    match origin {
        SearchOrigin::Analysis => 0,
        SearchOrigin::Fallback => 1,
        SearchOrigin::AlphaBeta => 2,
        SearchOrigin::Terminal => 3,
        SearchOrigin::Immediate => 4,
        SearchOrigin::Vcf => 5,
        SearchOrigin::Vct => 6,
        SearchOrigin::ProofBook => 7,
        SearchOrigin::OpeningBook => 8,
    }
}

fn decode_origin(tag: u8) -> Result<SearchOrigin, Box<dyn Error>> {
    Ok(match tag {
        0 => SearchOrigin::Analysis,
        1 => SearchOrigin::Fallback,
        2 => SearchOrigin::AlphaBeta,
        3 => SearchOrigin::Terminal,
        4 => SearchOrigin::Immediate,
        5 => SearchOrigin::Vcf,
        6 => SearchOrigin::Vct,
        7 => SearchOrigin::ProofBook,
        8 => SearchOrigin::OpeningBook,
        _ => return Err("invalid result-source tag".into()),
    })
}

const fn symmetry_tag(symmetry: Symmetry) -> u8 {
    match symmetry {
        Symmetry::Identity => 0,
        Symmetry::Rotate90 => 1,
        Symmetry::Rotate180 => 2,
        Symmetry::Rotate270 => 3,
        Symmetry::MirrorVertical => 4,
        Symmetry::MirrorHorizontal => 5,
        Symmetry::MirrorMainDiagonal => 6,
        Symmetry::MirrorAntiDiagonal => 7,
    }
}

const fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

struct Arguments {
    values: Vec<String>,
}

impl Arguments {
    fn new(values: impl Iterator<Item = String>) -> Self {
        Self {
            values: values.collect(),
        }
    }
    fn command(&mut self) -> Result<String, Box<dyn Error>> {
        if self.values.is_empty() {
            usage();
            return Err("a command is required".into());
        }
        Ok(self.values.remove(0))
    }
    fn take(&mut self, name: &str) -> Result<Option<String>, Box<dyn Error>> {
        let Some(index) = self.values.iter().position(|value| value == name) else {
            return Ok(None);
        };
        if index + 1 >= self.values.len() {
            return Err(format!("{name} requires a value").into());
        }
        self.values.remove(index);
        Ok(Some(self.values.remove(index)))
    }
    fn required(&mut self, name: &str) -> Result<String, Box<dyn Error>> {
        self.take(name)?
            .ok_or_else(|| format!("missing required {name}").into())
    }
    fn optional<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>, Box<dyn Error>>
    where
        T::Err: Error + 'static,
    {
        self.take(name)?
            .map(|value| {
                value
                    .parse()
                    .map_err(|error| Box::new(error) as Box<dyn Error>)
            })
            .transpose()
    }
    fn finish(self) -> Result<(), Box<dyn Error>> {
        if self.values.is_empty() {
            Ok(())
        } else {
            Err(format!("unexpected arguments: {}", self.values.join(" ")).into())
        }
    }
}

fn usage() {
    eprintln!(
        "Usage:\n  rustmoku-data record --record FILE --output FILE [--depth N] [--nodes N]\n  rustmoku-data selfplay --games N --seed N --output FILE [--workers N] [--depth N] [--nodes N]\n  rustmoku-data inspect --dataset FILE\n  rustmoku-data model-check --model FILE --record FILE [--move H8]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustmoku_core::RuleSet;

    #[test]
    fn selfplay_bytes_ignore_worker_partition_and_shard_boundaries() {
        let root = env::temp_dir().join(format!("rustmoku-stable-games-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let generate = |name: &str, first: usize, games: usize, workers: usize| {
            let path = root.join(name);
            let arguments = vec![
                "--output".to_string(),
                path.display().to_string(),
                "--games".to_string(),
                games.to_string(),
                "--seed".to_string(),
                "321".to_string(),
                "--first-game".to_string(),
                first.to_string(),
                "--workers".to_string(),
                workers.to_string(),
                "--depth".to_string(),
                "1".to_string(),
                "--nodes".to_string(),
                "64".to_string(),
                "--random-plies".to_string(),
                "2".to_string(),
            ];
            generate_selfplay(Arguments::new(arguments.into_iter())).unwrap();
            let records = read_dataset(&path).unwrap();
            std::fs::remove_file(path).unwrap();
            records
        };
        let baseline = generate("single.rmd", 17, 2, 1);
        assert_eq!(baseline, generate("parallel.rmd", 17, 2, 2));
        let mut shards = generate("first.rmd", 17, 1, 1);
        shards.extend(generate("second.rmd", 18, 1, 1));
        assert_eq!(baseline, shards);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn new_teacher_paths_preserve_exploration_identity_and_legal_branch() {
        let root = env::temp_dir().join(format!("rustmoku-teacher-paths-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut runs = Vec::new();
        for workers in [1, 2] {
            let output = root.join(format!("explore-{workers}.rmd"));
            let args = [
                "--games",
                "2",
                "--seed",
                "543",
                "--depth",
                "1",
                "--nodes",
                "500",
                "--explore-top-k",
                "3",
                "--explore-plies",
                "10",
                "--output",
                output.to_str().unwrap(),
                "--workers",
                if workers == 1 { "1" } else { "2" },
            ];
            generate_selfplay(Arguments::new(args.map(str::to_string).into_iter())).unwrap();
            let policy = output.with_extension("policy.jsonl");
            let labels = std::fs::read_to_string(&policy).unwrap();
            assert!(!labels.is_empty());
            assert!(
                labels
                    .lines()
                    .all(|line| line.contains("root-side-to-move"))
            );
            runs.push((read_dataset(&output).unwrap(), labels));
            std::fs::remove_file(output).unwrap();
            std::fs::remove_file(policy).unwrap();
        }
        assert_eq!(runs[0], runs[1]);
        let input = root.join("source.rmg");
        let source =
            Game::from_record("RustMoku 1\nrules=freestyle\nmoves=H8 A1 I8 A2 J8 B1\n").unwrap();
        std::fs::write(&input, source.to_record()).unwrap();
        let output = root.join("branch.rmd");
        let args = [
            "--record",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--branch-ply",
            "4",
            "--branch-choice",
            "0",
            "--depth",
            "1",
            "--nodes",
            "128",
        ];
        generate_record(Arguments::new(args.map(str::to_string).into_iter())).unwrap();
        let mut expected = Game::default();
        for at in source.history().take(4) {
            expected.play_move(at).unwrap();
        }
        let alternative = Move::all()
            .find(|&at| expected.position().is_legal(at) && Some(at) != source.history().nth(4))
            .unwrap();
        expected.play_move(alternative).unwrap();
        let rows = read_dataset(&output).unwrap();
        assert_eq!(
            rows.last().unwrap().position,
            CanonicalPosition::new(expected.position()).key()
        );
        assert_eq!(rows.last().unwrap().ply, 5);
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn offline_diversity_preserves_exact_wins_and_defenses() {
        for moves in ["H8 A1 I8 A2 J8 B1 K8 B2", "A1 H8 A3 I8 B1 J8 B3 K8"] {
            let game = Game::from_record(&format!("RustMoku 1\nrules=freestyle\nmoves={moves}\n"))
                .unwrap();
            let position = game.position();
            let teacher = Move::all()
                .find(|&at| {
                    position.is_legal(at)
                        && (position.would_win(at, position.side_to_move())
                            || position.would_win(at, position.side_to_move().opponent()))
                })
                .unwrap();
            for seed in 0..32 {
                assert_eq!(diverse_move(&game, teacher, seed), teacher);
            }
        }
    }

    #[test]
    fn dataset_round_trip_is_deterministic_and_checked() {
        let record = DataRecord {
            game_id: 7,
            ply: 3,
            canonical_symmetry: 0,
            policy_move: Some(Move::CENTER),
            value: 42,
            source: SearchOrigin::AlphaBeta,
            exact: false,
            position: CanonicalPosition::new(Game::new(RuleSet::Freestyle).position()).key(),
            quality: None,
        };
        let path = env::temp_dir().join(format!("rustmoku-data-{}.bin", std::process::id()));
        write_dataset(&path, &[record]).unwrap();
        let first = std::fs::read(&path).unwrap();
        assert_eq!(read_dataset(&path).unwrap(), vec![record]);
        write_dataset(&path, &[record]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), first);
        let mut malformed = first;
        malformed.push(0);
        std::fs::write(&path, malformed).unwrap();
        assert!(read_dataset(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn game_seed_derivation_is_stable_per_game_index() {
        assert_eq!(
            (0..4)
                .map(|game| splitmix64(123 ^ game))
                .collect::<Vec<_>>(),
            [
                0xb4dc_9bd4_62de_412b,
                0x1882_c195_e434_7c74,
                0x28bf_8a80_bc3e_ab52,
                0x02f5_9075_8a6d_2936,
            ]
        );
    }
}
