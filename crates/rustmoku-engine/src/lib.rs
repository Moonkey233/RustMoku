#![forbid(unsafe_code)]

mod config;
mod evaluation;
mod move_generation;
mod move_ordering;
mod principal_variation;
mod score;
mod search;
mod search_state;
mod transposition_table;
mod zobrist;

pub use config::EngineConfig;
pub use evaluation::{ClassicalEvaluator, Evaluator};
pub use search::{AlphaBetaEngine, SearchEngine, SearchLimits, SearchResult, SearchStatistics};
