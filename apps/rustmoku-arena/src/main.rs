#![forbid(unsafe_code)]

mod external;

use rustmoku_core::{CanonicalPosition, Game, GameStatus, OPENINGS, Stone};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, EngineConfig, LearnedEvaluator, LearnedModel,
    PatternEvaluator, SearchEngine, SearchLimits, SearchResult,
};
use std::{
    env,
    error::Error,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default)]
enum EvaluatorConfig {
    #[default]
    Pattern,
    Classical,
    Learned(PathBuf),
    External(PathBuf),
}

#[derive(Clone, Debug, Default)]
struct PlayerConfig {
    engine: EngineConfig,
    evaluator: EvaluatorConfig,
    external_args: Vec<String>,
}

struct Options {
    players: [PlayerConfig; 2],
    limits: SearchLimits,
    pairs: usize,
    pair_start: usize,
    leg: Option<usize>,
    clock: Option<Duration>,
    increment: Duration,
    opening_records: Vec<PathBuf>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, Box<dyn Error>> {
        let mut options = Self {
            players: std::array::from_fn(|_| PlayerConfig::default()),
            limits: SearchLimits::new(3),
            pairs: 1,
            pair_start: 0,
            leg: None,
            clock: None,
            increment: Duration::ZERO,
            opening_records: Vec::new(),
        };
        let mut args = args;
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--pairs" => options.pairs = value.parse()?,
                "--pair-start" => options.pair_start = value.parse()?,
                "--leg" => options.leg = Some(value.parse()?),
                "--move-ms" => {
                    options.limits.move_time = Some(Duration::from_millis(value.parse()?))
                }
                "--clock-ms" => options.clock = Some(Duration::from_millis(value.parse()?)),
                "--increment-ms" => options.increment = Duration::from_millis(value.parse()?),
                "--opening-record" => options.opening_records.push(value.into()),
                "--depth" => options.limits.max_depth = value.parse()?,
                "--nodes" => options.limits = options.limits.with_max_nodes(value.parse()?),
                _ => {
                    let (player, key) = if let Some(key) = flag.strip_prefix("--a-") {
                        (0, key)
                    } else if let Some(key) = flag.strip_prefix("--b-") {
                        (1, key)
                    } else {
                        return Err(format!("unknown option: {flag}").into());
                    };
                    let config = &mut options.players[player];
                    let mut tactical = config.engine.tactical();
                    match key {
                        "evaluator" => {
                            config.evaluator = match value.as_str() {
                                "pattern" => EvaluatorConfig::Pattern,
                                "classical" => EvaluatorConfig::Classical,
                                "learned" => EvaluatorConfig::Learned(PathBuf::new()),
                                _ => {
                                    return Err(
                                        "evaluator must be pattern, classical, or learned".into()
                                    );
                                }
                            };
                        }
                        "external" => config.evaluator = EvaluatorConfig::External(value.into()),
                        "external-arg" => config.external_args.push(value),
                        "model" => config.evaluator = EvaluatorConfig::Learned(value.into()),
                        "tt-mib" => {
                            config.engine = config.engine.with_tt_memory_mib(value.parse()?);
                        }
                        "threads" => {
                            config.engine = config.engine.with_threads(value.parse()?);
                        }
                        "interior-vcf" => {
                            let parts: Vec<&str> = value.split(':').collect();
                            if parts.len() != 3 {
                                return Err(
                                    "interior-vcf requires plies:probe-work:total-work".into()
                                );
                            }
                            config.engine = config.engine.with_interior_vcf(
                                rustmoku_engine::ProofLimits::new(
                                    parts[0].parse()?,
                                    parts[1].parse()?,
                                ),
                                parts[2].parse()?,
                            );
                        }
                        "disable" => {
                            let mut selection = config.engine.selectivity();
                            for name in value.split(',') {
                                match name {
                                    "all" => selection = rustmoku_engine::SelectivityConfig::OFF,
                                    "rfp" => selection.reverse_futility = false,
                                    "futility" => selection.futility = false,
                                    "razor" => selection.razoring = false,
                                    "lmp" => selection.lmp = false,
                                    "lmr" => selection.lmr = false,
                                    "iir" => selection.iir = false,
                                    "extension" => selection.threat_extension = false,
                                    _ => return Err("unknown selectivity ablation".into()),
                                }
                            }
                            config.engine = config.engine.with_selectivity(selection);
                        }
                        "vcf-plies" => tactical.vcf.max_plies = value.parse()?,
                        "vcf-nodes" => tactical.vcf.max_nodes = value.parse()?,
                        "vct-plies" => tactical.vct.max_plies = value.parse()?,
                        "vct-nodes" => tactical.vct.max_nodes = value.parse()?,
                        "vct-mib" => tactical.vct_table_memory_mib = value.parse()?,
                        _ => return Err(format!("unknown player option: {flag}").into()),
                    }
                    config.engine = config.engine.with_tactical(tactical);
                }
            }
        }
        let available = if options.opening_records.is_empty() {
            OPENINGS.len()
        } else {
            options.opening_records.len()
        };
        if options.pairs == 0
            || options
                .pair_start
                .checked_add(options.pairs)
                .is_none_or(|end| end > available)
        {
            return Err("requested pairs exceed the opening suite".into());
        }
        if options.leg.is_some_and(|leg| !(1..=2).contains(&leg)) {
            return Err("--leg must be 1 or 2".into());
        }
        if options.limits.max_depth == 0 {
            return Err("--depth must be positive; depth zero is analysis-only".into());
        }
        for player in &options.players {
            if matches!(player.evaluator, EvaluatorConfig::External(_))
                && (options.limits.move_time.is_none() || options.limits.max_nodes.is_some())
            {
                return Err(
                    "external engines require --move-ms and cannot use Rust fixed-work limits"
                        .into(),
                );
            }
            if matches!(&player.evaluator, EvaluatorConfig::Learned(path) if path.as_os_str().is_empty())
            {
                return Err("learned evaluator requires --a-model/--b-model FILE".into());
            }
        }
        Ok(options)
    }
}

// Dispatch once per move, preserving each evaluator's static recursive path.
enum Player {
    Pattern(AlphaBetaEngine),
    Classical(AlphaBetaEngine<ClassicalEvaluator>),
    Learned(AlphaBetaEngine<LearnedEvaluator>),
    External(external::ExternalPlayer),
}

impl Player {
    fn new(config: &PlayerConfig) -> Result<Self, Box<dyn Error>> {
        Ok(match &config.evaluator {
            EvaluatorConfig::External(path) => Self::External(external::ExternalPlayer::start(
                path,
                &config.external_args,
            )?),
            EvaluatorConfig::Classical => Self::Classical(AlphaBetaEngine::with_config(
                ClassicalEvaluator,
                config.engine,
            )),
            EvaluatorConfig::Pattern => Self::Pattern(AlphaBetaEngine::with_config(
                PatternEvaluator,
                config.engine,
            )),
            EvaluatorConfig::Learned(path) => {
                let model = Arc::new(LearnedModel::read_from_path(path)?);
                Self::Learned(AlphaBetaEngine::with_config(
                    LearnedEvaluator::new(model),
                    config.engine,
                ))
            }
        })
    }
    fn search(&mut self, game: &Game, limits: SearchLimits) -> SearchResult {
        match self {
            Self::Pattern(engine) => engine.search(game.position(), limits),
            Self::Classical(engine) => engine.search(game.position(), limits),
            Self::Learned(engine) => engine.search(game.position(), limits),
            Self::External(_) => unreachable!("external moves use the protocol adapter"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Winner {
    A,
    B,
    Draw,
}

impl Winner {
    fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::Draw => "draw",
        }
    }
}

fn player_for(side: Stone, a_color: Stone) -> usize {
    usize::from(side != a_color)
}

#[derive(Debug, PartialEq, Eq)]
struct GameResult {
    winner: Winner,
    plies: usize,
    moves: u64,
    work: u64,
    failure: Option<String>,
}

#[cfg(test)]
fn play(
    opening: &rustmoku_core::Opening,
    a_color: Stone,
    configs: &[PlayerConfig; 2],
    limits: SearchLimits,
) -> Result<GameResult, Box<dyn Error>> {
    play_game(
        &opening.game()?,
        a_color,
        configs,
        limits,
        None,
        Duration::ZERO,
    )
}

fn play_game(
    opening: &Game,
    a_color: Stone,
    configs: &[PlayerConfig; 2],
    limits: SearchLimits,
    clock: Option<Duration>,
    increment: Duration,
) -> Result<GameResult, Box<dyn Error>> {
    let mut game = Game::new(opening.position().rules());
    for at in opening.history() {
        game.play_move(at)?;
    }
    let mut clocks = [clock; 2];
    // Fresh per game, persistent between its moves. Paired legs cannot inherit
    // asymmetric ordinary TT history from one another.
    let mut players = Vec::with_capacity(2);
    for (index, config) in configs.iter().enumerate() {
        let startup = Instant::now();
        match Player::new(config) {
            Ok(player) => {
                players.push(player);
                if let Some(remaining) = &mut clocks[index] {
                    *remaining = remaining.saturating_sub(startup.elapsed());
                }
            }
            Err(error) => {
                return Ok(GameResult {
                    winner: if index == 0 { Winner::B } else { Winner::A },
                    plies: game.position().move_count(),
                    moves: 0,
                    work: 0,
                    failure: Some(format!("player-{index}-startup:{error}")),
                });
            }
        }
    }
    let (mut work, mut moves) = (0, 0);
    loop {
        let winner = match game.status() {
            GameStatus::Won(stone) if stone == a_color => Some(Winner::A),
            GameStatus::Won(_) => Some(Winner::B),
            GameStatus::Draw => Some(Winner::Draw),
            GameStatus::Ongoing => None,
        };
        if let Some(winner) = winner {
            return Ok(GameResult {
                winner,
                plies: game.position().move_count(),
                moves,
                work,
                failure: None,
            });
        }
        let player = player_for(game.position().side_to_move(), a_color);
        let allocation = allocate_time(limits.move_time, clocks[player]);
        let hard_limit = match (limits.move_time, clocks[player]) {
            (Some(turn), Some(clock)) => Some(turn.min(clock)),
            (turn, clock) => turn.or(clock),
        };
        let mut move_limits = limits;
        move_limits.move_time = allocation.map(|time| time.mul_f64(0.95));
        let start = Instant::now();
        let choice = match &mut players[player] {
            Player::External(external) => external
                .choose(
                    &game,
                    move_limits
                        .move_time
                        .expect("validated external time limit"),
                    hard_limit.expect("validated external time limit"),
                )
                .map(|at| (at, 0)),
            internal => {
                let result = internal.search(&game, move_limits);
                result
                    .best_move
                    .map(|at| (at, result.statistics.work_nodes))
                    .ok_or_else(|| "no move in ongoing game".to_owned())
            }
        };
        let elapsed = start.elapsed();
        let choice = if hard_limit.is_some_and(|time| elapsed > time) {
            Err("time-forfeit".to_owned())
        } else {
            choice
        };
        let (at, used_work) = match choice {
            Ok(choice) => choice,
            Err(reason) => {
                return Ok(GameResult {
                    winner: if player == 0 { Winner::B } else { Winner::A },
                    plies: game.position().move_count(),
                    moves,
                    work,
                    failure: Some(format!("player-{player}:{reason}")),
                });
            }
        };
        game.play_move(at)?;
        if let Some(remaining) = &mut clocks[player] {
            *remaining = remaining.saturating_sub(elapsed).saturating_add(increment);
        }
        work += used_work;
        moves += 1;
    }
}

// Application-side clock allocation; Engine receives only a per-search limit.
fn allocate_time(per_move: Option<Duration>, remaining: Option<Duration>) -> Option<Duration> {
    match (per_move, remaining) {
        (Some(turn), Some(clock)) => Some(turn.min(clock / 20)),
        (None, Some(clock)) => Some(clock / 20),
        (turn, None) => turn,
    }
}

#[derive(Default)]
struct Summary {
    a: u64,
    b: u64,
    draws: u64,
    moves: u64,
    work: u64,
}

impl Summary {
    fn record(&mut self, result: &GameResult) {
        match result.winner {
            Winner::A => self.a += 1,
            Winner::B => self.b += 1,
            Winner::Draw => self.draws += 1,
        }
        self.moves += result.moves;
        self.work += result.work;
    }
    fn a_points(&self) -> f64 {
        self.a as f64 + self.draws as f64 / 2.0
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().any(|arg| arg == "--help") {
        println!(
            "RustMoku V0.12 Arena\n--pairs 1..12 --depth N --nodes N (optional global work cap per move)\nPlayer flags: --a- or --b- followed by evaluator pattern|classical|learned, model FILE, threads N,\ntt-mib N, vcf-plies N, vcf-nodes N, vct-plies N, vct-nodes N, vct-mib N.\nZero proof nodes/plies disables that solver. CSV stdout; configuration/summary stderr."
        );
        return Ok(());
    }
    let options = Options::parse(env::args().skip(1))?;
    eprintln!(
        "RustMoku V0.12 Arena: {:?}; A={:?}; B={:?}",
        options.limits, options.players[0], options.players[1]
    );
    println!("pair,opening,leg,a_color,winner,plies,searched_moves,work_nodes,opening_key,failure");
    let mut summary = Summary::default();
    let openings = if options.opening_records.is_empty() {
        OPENINGS
            .iter()
            .map(|opening| Ok((opening.id.to_string(), opening.game()?)))
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?
    } else {
        options
            .opening_records
            .iter()
            .enumerate()
            .map(|(i, path)| {
                Ok((
                    format!("custom-{}", i + 1),
                    Game::from_record(&std::fs::read_to_string(path)?)?,
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?
    };
    for (pair, (opening_id, opening)) in openings
        .iter()
        .enumerate()
        .skip(options.pair_start)
        .take(options.pairs)
    {
        let opening_key = CanonicalPosition::new(opening.position())
            .key()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        for (leg, a_color) in [Stone::Black, Stone::White].into_iter().enumerate() {
            if options.leg.is_some_and(|selected| selected != leg + 1) {
                continue;
            }
            let result = play_game(
                opening,
                a_color,
                &options.players,
                options.limits,
                options.clock,
                options.increment,
            )?;
            println!(
                "{},{},{},{:?},{},{},{},{},{},{}",
                pair + 1,
                opening_id,
                leg + 1,
                a_color,
                result.winner.label(),
                result.plies,
                result.moves,
                result.work,
                opening_key,
                result
                    .failure
                    .as_deref()
                    .unwrap_or("")
                    .replace([',', '\n', '\r'], " ")
            );
            summary.record(&result);
        }
    }
    eprintln!(
        "A wins: {}; B wins: {}; draws: {}; A paired score: {:.1}/{} ({:.3} points/pair); average work nodes/move: {:.1}",
        summary.a,
        summary.b,
        summary.draws,
        summary.a_points(),
        options.pairs * 2,
        summary.a_points() / options.pairs as f64,
        summary.work as f64 / summary.moves.max(1) as f64
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_colors_and_accounting_use_the_same_legal_opening() {
        let config = PlayerConfig {
            engine: EngineConfig::new(0).with_vct_table_memory(0),
            evaluator: EvaluatorConfig::Pattern,
            external_args: Vec::new(),
        };
        let limits = SearchLimits::new(1).with_max_nodes(100);
        let configs = [config.clone(), config];
        let black = play(&OPENINGS[0], Stone::Black, &configs, limits).unwrap();
        let white = play(&OPENINGS[0], Stone::White, &configs, limits).unwrap();
        assert_eq!(
            (black.plies, black.moves, black.work),
            (white.plies, white.moves, white.work)
        );
        assert_eq!(black.winner == Winner::A, white.winner == Winner::B);
        assert_eq!(player_for(Stone::Black, Stone::White), 1);
        let mut summary = Summary::default();
        summary.record(&black);
        summary.record(&white);
        assert_eq!(summary.a_points(), 1.0);
        assert_eq!(summary.a + summary.b + summary.draws, 2);
        summary.record(&GameResult {
            winner: Winner::Draw,
            plies: 225,
            moves: 0,
            work: 0,
            failure: None,
        });
        assert_eq!(summary.a_points(), 1.5);
    }

    #[test]
    fn player_configuration_and_limit_options_are_independent() {
        let options = Options::parse(
            [
                "--a-tt-mib",
                "1",
                "--b-vct-nodes",
                "0",
                "--a-vcf-nodes",
                "17",
                "--a-threads",
                "4",
                "--b-evaluator",
                "classical",
                "--nodes",
                "500",
                "--pairs",
                "2",
            ]
            .map(String::from)
            .into_iter(),
        )
        .unwrap();
        assert_eq!(options.players[0].engine.tt_memory_mib(), 1);
        assert_eq!(options.players[0].engine.vcf_max_nodes(), 17);
        assert_eq!(options.players[0].engine.threads(), 4);
        assert!(options.players[0].engine.tactical().vct.enabled());
        assert!(!options.players[1].engine.tactical().vct.enabled());
        assert!(matches!(
            options.players[1].evaluator,
            EvaluatorConfig::Classical
        ));
        assert_eq!(options.limits.max_nodes, Some(500));
        assert_eq!(options.pairs, 2);
        assert!(Options::parse(["--depth", "0"].map(String::from).into_iter()).is_err());
    }

    #[test]
    fn threaded_players_still_replay_the_opening_and_finish_legally() {
        let config = PlayerConfig {
            engine: EngineConfig::new(0)
                .with_threads(2)
                .with_vcf_limits(0, 0)
                .with_vct_limits(0, 0)
                .with_vct_table_memory(0),
            evaluator: EvaluatorConfig::Pattern,
            external_args: Vec::new(),
        };
        let configs = [config.clone(), config];
        let result = play(
            &OPENINGS[0],
            Stone::Black,
            &configs,
            SearchLimits::new(1).with_max_nodes(100),
        )
        .unwrap();
        assert_eq!(
            result.plies,
            OPENINGS[0].moves.len() + result.moves as usize
        );
        assert!(result.moves > 0);
        assert!(result.work > 0);
    }
}
