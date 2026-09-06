#![forbid(unsafe_code)]

/// Explicit opt-in benchmark driver; not part of the normal engine API.
#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub mod benchmarks;

mod bitboard;
mod board_state;
mod candidate_frontier;
mod config;
mod evaluation;
#[cfg(test)]
mod line_classifier;
mod line_geometry;
mod move_generation;
mod move_ordering;
mod offline;
mod pattern;
mod pattern_state;
mod principal_variation;
mod proof_book;
mod proof_table;
mod score;
mod search;
mod search_control;
mod search_heuristics;
mod search_params;
mod search_state;
mod tactical;
mod transposition_table;
mod vcf;
mod vct;
mod zobrist;

pub use config::{EngineConfig, ProofLimits, TacticalConfig};
pub use evaluation::{ClassicalEvaluator, Evaluator, PatternEvaluator};
pub use offline::{
    OfflineSolver, ProofOutcome, SolverError, SolverLimits, SolverResult, SolverStatistics,
    SolverTermination,
};
pub use pattern_state::PatternState;
pub use proof_book::{
    Proof, ProofBook, ProofBookError, ProofBookHit, ProofBookMetadata, ProofBookSourceSummary,
    ProofDistance, ProofSource, VerifiedProofBook,
};
pub use search::{
    AlphaBetaEngine, SearchEngine, SearchInfo, SearchLimits, SearchObserver, SearchResult,
    SearchStatistics,
};
pub use search_control::{CancellationToken, SearchTermination};
pub use transposition_table::TranspositionTableStatistics;
