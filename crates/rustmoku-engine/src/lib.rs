#![forbid(unsafe_code)]

/// Explicit opt-in benchmark driver; not part of the normal engine API.
#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub mod benchmarks;

mod bitboard;
mod candidate_frontier;
mod config;
mod evaluation;
mod line_geometry;
mod move_generation;
mod move_ordering;
mod pattern;
mod pattern_state;
mod principal_variation;
mod score;
mod search;
mod search_heuristics;
mod search_state;
mod tactical;
mod transposition_table;
mod zobrist;

pub use config::EngineConfig;
pub use evaluation::{ClassicalEvaluator, Evaluator, PatternEvaluator};
pub use pattern_state::{PatternState, PatternUndo};
pub use search::{AlphaBetaEngine, SearchEngine, SearchLimits, SearchResult, SearchStatistics};
pub use transposition_table::TranspositionTableStatistics;
