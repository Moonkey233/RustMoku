#![forbid(unsafe_code)]

use std::{
    env,
    error::Error,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    thread,
};

use rustmoku_core::{
    CELL_COUNT, CanonicalPosition, CanonicalPositionKey, Game, Move, OPENINGS, Symmetry,
};
use rustmoku_engine::{
    AlphaBetaEngine, EngineConfig, LearnedEvaluator, LearnedModel, PatternEvaluator, ProofBook,
    ProofBookVerifyLimits, ProofDistance, SearchEngine, SearchLimits, SearchOrigin,
    SearchTermination,
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
        "record" => generate_record(args),
        "proof" => generate_proof(args),
        "selfplay" => generate_selfplay(args),
        "inspect" => inspect(args),
        "model-check" => model_check(args),
        "help" | "--help" | "-h" => {
            usage();
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
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
    std::fs::write(
        output.with_extension("proof.json"),
        format!("{{\"version\":1,\"games\":{{{}}}}}\n", metadata.join(",")),
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
    args.finish()?;
    let source = Game::from_record(&std::fs::read_to_string(input)?)?;
    let mut trajectory = Game::new(source.position().rules());
    let mut teacher = teacher_engine();
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

fn generate_selfplay(mut args: Arguments) -> Result<(), Box<dyn Error>> {
    let output = PathBuf::from(args.required("--output")?);
    let games: usize = args.required("--games")?.parse()?;
    let seed: u64 = args.required("--seed")?.parse()?;
    let requested_workers: usize = args.optional("--workers")?.unwrap_or(1);
    let depth = args.optional("--depth")?.unwrap_or(6);
    let nodes = args.optional("--nodes")?.unwrap_or(20_000);
    let random_plies: usize = args.optional("--random-plies")?.unwrap_or(0);
    let cold_games: bool = args.optional("--cold-games")?.unwrap_or(false);
    args.finish()?;
    if random_plies > 8 {
        return Err("random prefix must be 0..=8 plies".into());
    }
    if games == 0 || games > MAX_RECORDS / (CELL_COUNT + 1) {
        return Err("game count can exceed the dataset record safety limit".into());
    }
    if requested_workers == 0 || requested_workers > 256 {
        return Err("workers must be in 1..=256".into());
    }
    let workers = requested_workers.min(games);
    let mut records = thread::scope(|scope| {
        let mut handles = Vec::new();
        for worker in 0..workers {
            handles.push(scope.spawn(move || -> Result<Vec<DataRecord>, String> {
                // Independent persistent teacher state: no globally locked
                // engine and no scheduler-dependent cross-worker TT sharing.
                let mut teacher = teacher_engine();
                let mut records = Vec::new();
                for game_id in (worker..games).step_by(workers) {
                    if cold_games {
                        teacher = teacher_engine();
                    }
                    let opening_index =
                        (splitmix64(seed ^ game_id as u64) as usize) % OPENINGS.len();
                    let mut game = OPENINGS[opening_index]
                        .game()
                        .map_err(|error| error.to_string())?;
                    let opening_plies = game.history().len();
                    while game.status() == rustmoku_core::GameStatus::Ongoing {
                        let record = label_position(
                            &mut teacher,
                            game_id as u64,
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
                        let at = if game.history().len() - opening_plies < random_plies {
                            diverse_move(
                                &game,
                                at,
                                splitmix64(
                                    seed ^ ((game_id as u64) << 32) ^ game.history().len() as u64,
                                ),
                            )
                        } else {
                            at
                        };
                        records.push(record);
                        game.play_move(at).map_err(|error| error.to_string())?;
                    }
                    records.push(
                        label_position(
                            &mut teacher,
                            game_id as u64,
                            game.history().len(),
                            &game,
                            depth,
                            nodes,
                        )
                        .map_err(|error| error.to_string())?,
                    );
                }
                Ok(records)
            }));
        }
        let mut records = Vec::new();
        for handle in handles {
            records.extend(handle.join().expect("data worker panicked")?);
        }
        Ok::<_, String>(records)
    })?;
    records.sort_by_key(|record| (record.game_id, record.ply));
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

fn label_position(
    engine: &mut AlphaBetaEngine<PatternEvaluator>,
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

fn teacher_engine() -> AlphaBetaEngine<PatternEvaluator> {
    AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(64).with_threads(1))
}

fn write_dataset(path: &Path, records: &[DataRecord]) -> Result<(), Box<dyn Error>> {
    if records.len() > MAX_RECORDS {
        return Err("dataset record count exceeds safety limit".into());
    }
    let mut file = File::create(path)?;
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
    args.finish()?;
    let game = Game::from_record(&std::fs::read_to_string(record)?)?;
    let evaluator = LearnedEvaluator::new(Arc::new(LearnedModel::read_from_path(model)?));
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
