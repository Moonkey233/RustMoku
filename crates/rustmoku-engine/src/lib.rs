#![forbid(unsafe_code)]

/// Explicit opt-in benchmark driver; not part of the normal engine API.
#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub mod benchmarks;

mod bitboard;
mod board_state;
mod candidate_frontier;
mod candidate_universe;
mod config;
mod evaluation;
mod interior_proof;
mod learned;
#[cfg(test)]
mod line_classifier;
mod line_geometry;
mod move_generation;
mod move_ordering;
mod nonlinear;
/// Offline float architecture oracle; never a runtime evaluator default.
#[cfg(feature = "experimental-v2")]
pub mod nonlinear_reference;
mod offline;
mod pattern;
mod pattern_state;
mod principal_variation;
mod probcut;
mod proof_book;
mod proof_table;
mod score;
mod search;
mod search_control;
mod search_heuristics;
mod search_params;
mod search_profile;
mod search_state;
mod tactical;
mod transposition_table;
mod vcf;
mod vct;
mod zobrist;

pub use candidate_universe::{
    ProductionCandidateUniverse, ProofCandidateUniverse, TeacherCandidateUniverse,
    TeacherCandidates,
};
pub use config::{EngineConfig, ProofLimits, SelectivityConfig, TacticalConfig};
pub use evaluation::{ClassicalEvaluator, Evaluator, PatternEvaluator};
pub use interior_proof::InteriorProofStatistics;
pub use learned::{
    LEARNED_FEATURE_COUNT, LEARNED_HIDDEN, LearnedEvaluator, LearnedModel, LearnedModelError,
    LearnedModelMetadata, LearnedState, RuntimeEvaluator, RuntimeEvaluatorState,
};
pub use nonlinear::{NONLINEAR_WIDTH, NonlinearEvaluator, NonlinearModel, NonlinearState};
pub use offline::{
    MAX_PERSISTED_SOLVER_NODES, OfflineSolver, ProofOutcome, SolverError, SolverLimits,
    SolverResult, SolverStatistics, SolverTermination,
};
pub use pattern_state::{PatternDelta, PatternState};
pub use probcut::{ProbCutBucket, ProbCutCalibration};
pub use proof_book::{
    Proof, ProofBook, ProofBookError, ProofBookHit, ProofBookMetadata, ProofBookSourceSummary,
    ProofBookVerifyLimits, ProofDistance, ProofSource, ProofTrainingSample, VerifiedProofBook,
};
pub use search::{
    AlphaBetaEngine, CandidateBound, RootAnalysis, RootCandidate, ScoreAnalysis, SearchEngine,
    SearchInfo, SearchLimits, SearchObserver, SearchOrigin, SearchResult, SearchStatistics,
};
pub use search_control::{CancellationToken, SearchTermination};
pub use search_profile::{ScoreContract, SearchParameters, SearchProfile};
pub use transposition_table::TranspositionTableStatistics;
