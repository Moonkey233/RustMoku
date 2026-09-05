#![forbid(unsafe_code)]

use rustmoku_core::{Game, GameStatus, OPENINGS, Opening, Stone};
use rustmoku_engine::{
    AlphaBetaEngine, ClassicalEvaluator, EngineConfig, PatternEvaluator, SearchEngine,
    SearchLimits, SearchResult,
};
use std::{env, error::Error};

#[derive(Clone, Copy, Debug, Default)]
struct PlayerConfig {
    engine: EngineConfig,
    classical: bool,
}

struct Options {
    players: [PlayerConfig; 2],
    limits: SearchLimits,
    pairs: usize,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, Box<dyn Error>> {
        let mut options = Self {
            players: [PlayerConfig::default(); 2],
            limits: SearchLimits::new(3),
            pairs: 1,
        };
        let mut args = args;
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--pairs" => options.pairs = value.parse()?,
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
                            config.classical = match value.as_str() {
                                "pattern" => false,
                                "classical" => true,
                                _ => return Err("evaluator must be pattern or classical".into()),
                            }
                        }
                        "tt-mib" => {
                            config.engine =
                                EngineConfig::new(value.parse()?).with_tactical(tactical)
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
        if !(1..=OPENINGS.len()).contains(&options.pairs) {
            return Err("--pairs must be 1..=12 (fixed opening prefixes)".into());
        }
        if options.limits.max_depth == 0 {
            return Err("--depth must be positive; depth zero is analysis-only".into());
        }
        Ok(options)
    }
}

// Dispatch once per move, preserving each evaluator's static recursive path.
enum Player {
    Pattern(AlphaBetaEngine),
    Classical(AlphaBetaEngine<ClassicalEvaluator>),
}

impl Player {
    fn new(config: PlayerConfig) -> Self {
        if config.classical {
            Self::Classical(AlphaBetaEngine::with_config(
                ClassicalEvaluator,
                config.engine,
            ))
        } else {
            Self::Pattern(AlphaBetaEngine::with_config(
                PatternEvaluator,
                config.engine,
            ))
        }
    }
    fn search(&mut self, game: &Game, limits: SearchLimits) -> SearchResult {
        match self {
            Self::Pattern(engine) => engine.search(game.position(), limits),
            Self::Classical(engine) => engine.search(game.position(), limits),
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
}

fn play(
    opening: &Opening,
    a_color: Stone,
    configs: [PlayerConfig; 2],
    limits: SearchLimits,
) -> Result<GameResult, Box<dyn Error>> {
    let mut game = opening.game()?;
    // Fresh per game, persistent between its moves. Paired legs cannot inherit
    // asymmetric ordinary TT history from one another.
    let mut players = configs.map(Player::new);
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
            });
        }
        let result =
            players[player_for(game.position().side_to_move(), a_color)].search(&game, limits);
        let at = result
            .best_move
            .ok_or("engine returned no move in an ongoing game")?;
        game.play_move(at)?;
        work += result.statistics.work_nodes;
        moves += 1;
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
            "RustMoku V0.8 Arena\n--pairs 1..12 --depth N --nodes N (optional global work cap per move)\nPlayer flags: --a- or --b- followed by evaluator pattern|classical, tt-mib N,\nvcf-plies N, vcf-nodes N, vct-plies N, vct-nodes N, vct-mib N.\nZero proof nodes/plies disables that solver. CSV stdout; configuration/summary stderr."
        );
        return Ok(());
    }
    let options = Options::parse(env::args().skip(1))?;
    eprintln!(
        "RustMoku V0.8 Arena: {:?}; A={:?}; B={:?}",
        options.limits, options.players[0], options.players[1]
    );
    println!("pair,opening,leg,a_color,winner,plies,searched_moves,work_nodes");
    let mut summary = Summary::default();
    for (pair, opening) in OPENINGS.iter().take(options.pairs).enumerate() {
        eprintln!(
            "Opening {} ({}): {}",
            opening.id,
            opening.name,
            opening
                .moves
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
        for (leg, a_color) in [Stone::Black, Stone::White].into_iter().enumerate() {
            let result = play(opening, a_color, options.players, options.limits)?;
            println!(
                "{},{},{},{:?},{},{},{},{}",
                pair + 1,
                opening.id,
                leg + 1,
                a_color,
                result.winner.label(),
                result.plies,
                result.moves,
                result.work
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
            classical: false,
        };
        let limits = SearchLimits::new(1).with_max_nodes(100);
        let black = play(&OPENINGS[0], Stone::Black, [config; 2], limits).unwrap();
        let white = play(&OPENINGS[0], Stone::White, [config; 2], limits).unwrap();
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
        assert!(options.players[0].engine.tactical().vct.enabled());
        assert!(!options.players[1].engine.tactical().vct.enabled());
        assert!(options.players[1].classical);
        assert_eq!(options.limits.max_nodes, Some(500));
        assert_eq!(options.pairs, 2);
        assert!(Options::parse(["--depth", "0"].map(String::from).into_iter()).is_err());
    }
}
