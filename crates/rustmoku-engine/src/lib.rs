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
mod learned;
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
pub use learned::{
    LEARNED_FEATURE_COUNT, LEARNED_HIDDEN, LearnedEvaluator, LearnedModel, LearnedModelError,
    LearnedModelMetadata, LearnedState, RuntimeEvaluator, RuntimeEvaluatorState,
};
pub use offline::{
    MAX_PERSISTED_SOLVER_NODES, OfflineSolver, ProofOutcome, SolverError, SolverLimits,
    SolverResult, SolverStatistics, SolverTermination,
};
pub use pattern_state::{PatternDelta, PatternState};
pub use proof_book::{
    Proof, ProofBook, ProofBookError, ProofBookHit, ProofBookMetadata, ProofBookSourceSummary,
    ProofBookVerifyLimits, ProofDistance, ProofSource, VerifiedProofBook,
};
pub use search::{
    AlphaBetaEngine, SearchEngine, SearchInfo, SearchLimits, SearchObserver, SearchOrigin,
    SearchResult, SearchStatistics,
};
pub use search_control::{CancellationToken, SearchTermination};
pub use transposition_table::TranspositionTableStatistics;
