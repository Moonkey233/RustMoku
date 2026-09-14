mod alphabeta;
mod analysis;
mod bounds;
mod qsearch;
mod root;
mod selectivity;
use bounds::{BoundValidity, NodeResult};

use rustmoku_core::{Move, Position};
use std::{sync::Arc, time::Duration};

use crate::{
    CancellationToken, EngineConfig, Evaluator, PatternEvaluator, Proof, ProofDistance,
    ProofSource, SearchTermination, VerifiedProofBook,
    move_generation::MoveList,
    move_ordering::{order_moves, resistance_key},
    pattern::ThreatProfile,
    principal_variation::PvTable,
    score::{MATE_SCORE, MATE_THRESHOLD, SEARCH_INFINITY, score_from_tt, score_to_tt},
    search_control::{SearchBudget, Stopped},
    search_heuristics::SearchHeuristics,
    search_params,
    search_state::SearchState,
    tactical::{forcing_moves, immediate_tactic},
    transposition_table::{Bound, TranspositionTable, TranspositionTableStatistics, TtEntry},
    vcf::{VcfSolver, VcfStatus},
    vct::{VctSolver, VctStatus},
};

const MAX_QSEARCH_PLY: u8 = 6;

#[derive(Clone, Copy)]
struct QContext {
    qply: u8,
    active: Option<crate::tactical::ThreatDescriptor>,
}

#[cfg(test)]
#[path = "search_lifecycle_tests.rs"]
mod lifecycle_tests;
#[cfg(test)]
#[path = "research_tests.rs"]
mod research_tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_depth: u8,
    /// Total logical work across Alpha-Beta, qsearch, VCF and VCT.
    pub max_nodes: Option<u64>,
    /// Elapsed time for this public search, including root proofs.
    pub move_time: Option<Duration>,
}

impl SearchLimits {
    pub const DEFAULT_DEPTH: u8 = 4;

    #[must_use]
    pub const fn new(max_depth: u8) -> Self {
        Self {
            max_depth,
            max_nodes: None,
            move_time: None,
        }
    }

    #[must_use]
    pub const fn with_max_nodes(mut self, max_nodes: u64) -> Self {
        self.max_nodes = Some(max_nodes);
        self
    }

    #[must_use]
    pub const fn with_move_time(mut self, move_time: Duration) -> Self {
        self.move_time = Some(move_time);
        self
    }
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DEPTH)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStatistics {
    pub interior_proof: crate::InteriorProofStatistics,
    /// Total admitted logical visits, including proof certificate visits.
    /// `qnodes` is already included in `nodes`, and is not charged twice.
    pub work_nodes: u64,
    /// Alpha-Beta nodes, including qnodes and re-search work; proofs are separate.
    pub nodes: u64,
    pub qnodes: u64,
    /// Qsearch visits below the initial depth-zero replacement node.
    pub qsearch_recursive_nodes: u64,
    pub qsearch_forcing_edges: u64,
    pub qsearch_forced_blocks: u64,
    pub qsearch_stand_pat_cutoffs: u64,
    pub qsearch_cap_hits: u64,
    pub qsearch_three_edges: u64,
    pub qsearch_dependency_edges: u64,
    pub max_qply: u8,
    pub pvs_researches: u64,
    /// Verified equal mate-loss root scores resolved by practical preference.
    pub root_resistance_ties: u64,
    pub root_resistance_researches: u64,
    pub root_candidates_added: u64,
    pub lmr_reductions: u64,
    pub lmr_researches: u64,
    pub policy_lmr_reductions: u64,
    pub policy_lmr_researches: u64,
    pub policy_lmr_failed_verifications: u64,
    pub singular_attempts: u64,
    pub singular_extensions: u64,
    pub singular_incomplete: u64,
    pub singular_work: u64,
    pub probcut_attempts: u64,
    pub probcut_cutoffs: u64,
    pub probcut_work: u64,
    pub probcut_unqualified: u64,
    pub lmp_pruned_moves: u64,
    pub futility_pruned_moves: u64,
    pub rfp_attempts: u64,
    pub rfp_cutoffs: u64,
    pub razor_attempts: u64,
    pub razor_cutoffs: u64,
    pub null_attempts: u64,
    pub null_verifications: u64,
    pub null_cutoffs: u64,
    pub null_work: u64,
    pub iid_attempts: u64,
    pub iid_work: u64,
    pub policy_pruned_moves: u64,
    pub iir_reductions: u64,
    pub threat_extensions: u64,
    pub aspiration_fail_low: u64,
    pub aspiration_fail_high: u64,
    pub static_evaluations: u64,
    pub beta_cutoffs: u64,
    pub tt_probes: u64,
    pub tt_hits: u64,
    pub tt_cutoffs: u64,
    pub tt_stores: u64,
    pub tt_replacements: u64,
    pub vcf_nodes: u64,
    pub vcf_cache_hits: u64,
    /// Gated solver attempts, not proof-table lookups.
    pub vcf_probes: u64,
    pub vcf_proven: u64,
    pub vcf_budget_exhausted: u64,
    pub vct_nodes: u64,
    pub vct_probes: u64,
    pub vct_cache_hits: u64,
    pub vct_proven: u64,
    pub vct_budget_exhausted: u64,
    pub proof_book_probes: u64,
    pub proof_book_hits: u64,
    /// Configured worker count for this public search.
    pub worker_count: usize,
    /// Alpha-Beta nodes searched by the principal worker.
    pub principal_nodes: u64,
    /// Alpha-Beta nodes searched by all helper workers.
    pub helper_nodes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub best_move: Option<Move>,
    pub score: i32,
    pub requested_depth: u8,
    pub completed_depth: u8,
    pub seldepth: u8,
    pub principal_variation: Vec<Move>,
    pub statistics: SearchStatistics,
    pub proof: Option<Proof>,
    pub origin: SearchOrigin,
    pub termination: SearchTermination,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchOrigin {
    Analysis,
    #[default]
    Fallback,
    AlphaBeta,
    Terminal,
    Immediate,
    Vcf,
    Vct,
    ProofBook,
    OpeningBook,
}

/// A completed iteration or exact tactical proof, never a partial aspiration PV.
/// Scores use the root side-to-move perspective. Statistics are cumulative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchInfo {
    pub completed_depth: u8,
    pub seldepth: u8,
    pub best_move: Option<Move>,
    pub score: i32,
    pub principal_variation: Vec<Move>,
    pub statistics: SearchStatistics,
    pub proof: Option<Proof>,
    pub origin: SearchOrigin,
}

/// Research-only fixed-horizon analysis, not a proof or solved minimax value.
/// Practical uses all legal roots and radius-two descendants; AllLegal is a shallow oracle.
/// Both use the fixed Four-class qsearch leaf policy, not experimental Three hints.
/// Scores are always from `side_to_move` at the
/// supplied root; incomplete candidates have no score. All scored candidates
/// share a completed horizon and were searched with full windows, no depth pruning
/// and no ordinary TT access. Storage is allocated once at this public boundary.
#[derive(Clone, Debug)]
pub struct RootAnalysis {
    pub universe: crate::TeacherCandidates,
    pub score_contract: crate::ScoreContract,
    pub side_to_move: rustmoku_core::Stone,
    pub candidates: Vec<RootCandidate>,
    pub completed_depth: u8,
    pub termination: SearchTermination,
    pub work: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateBound {
    Unknown,
    /// Exact only within the declared candidate domain, horizon and leaf policy.
    DomainExact,
}

#[derive(Clone, Copy, Debug)]
pub struct RootCandidate {
    pub at: Move,
    pub score: Option<i32>,
    pub bound: CandidateBound,
    pub completed_depth: u8,
    pub nominal_depth_valid: bool,
    pub source: SearchOrigin,
    pub termination: SearchTermination,
    pub work: u64,
}

/// Exact fixed-horizon analysis for calibration, with no selective or ordinary TT authority.
#[derive(Clone, Debug)]
pub struct ScoreAnalysis {
    pub score: Option<i32>,
    pub requested_depth: u8,
    pub completed_depth: u8,
    pub termination: SearchTermination,
    pub work: u64,
    pub quiet: bool,
}

impl From<&SearchResult> for SearchInfo {
    fn from(result: &SearchResult) -> Self {
        Self {
            completed_depth: result.completed_depth,
            seldepth: result.seldepth,
            best_move: result.best_move,
            score: result.score,
            principal_variation: result.principal_variation.clone(),
            statistics: result.statistics,
            proof: result.proof,
            origin: result.origin,
        }
    }
}

/// Called only at completed root events; dispatch is outside recursive search.
pub trait SearchObserver {
    fn on_info(&mut self, info: SearchInfo);
    /// A soft decision at a completed iteration boundary. Hard deadlines and
    /// cancellation remain under SearchBudget; this is a successful completion.
    fn should_stop(&mut self) -> bool {
        false
    }
}

impl<F: FnMut(SearchInfo)> SearchObserver for F {
    fn on_info(&mut self, info: SearchInfo) {
        self(info);
    }
}

pub trait SearchEngine {
    fn search(&mut self, position: &Position, limits: SearchLimits) -> SearchResult {
        self.search_controlled(position, limits, CancellationToken::new(), &mut |_| {})
    }

    /// Caller retains a clone of the one-way token when cancellation is needed.
    /// If interrupted, returns the last completed iteration. Before any depth
    /// completes, a nonterminal positive-depth search uses tactical/policy/center
    /// preference (center on an empty board), static score and one-move fallback PV.
    /// Zero depth remains analysis-only with no move. Exact root tactics remain
    /// valid completed results. Cancelled application requests must not be played.
    fn search_controlled(
        &mut self,
        position: &Position,
        limits: SearchLimits,
        cancellation: CancellationToken,
        observer: &mut dyn SearchObserver,
    ) -> SearchResult;
}

pub struct AlphaBetaEngine<E = PatternEvaluator> {
    evaluator: E,
    table: TranspositionTable,
    generation: u8,
    config: EngineConfig,
    vcf: VcfSolver,
    vct: VctSolver,
    proof_book: Option<Arc<VerifiedProofBook>>,
    scratch: Option<WorkerScratch>,
    helper_scratch: Vec<WorkerScratch>,
    opening_database: Option<(Arc<crate::OpeningDatabase>, crate::OpeningPolicy)>,
}

impl<E> AlphaBetaEngine<E> {
    #[must_use]
    pub fn new(evaluator: E) -> Self {
        Self::with_config(evaluator, EngineConfig::default())
    }

    #[must_use]
    pub fn with_config(evaluator: E, config: EngineConfig) -> Self {
        Self {
            evaluator,
            table: TranspositionTable::new(config.tt_memory_mib()),
            generation: 0,
            config,
            vcf: VcfSolver::new(),
            vct: VctSolver::new(config.tactical().vct_table_memory_mib),
            proof_book: None,
            scratch: None,
            helper_scratch: Vec::new(),
            opening_database: None,
        }
    }

    pub fn clear_transposition_table(&mut self) {
        self.table.clear();
        self.generation = 0;
    }

    /// Replace the evaluator definition between searches. Ordinary TT scores
    /// are evaluator-dependent and are therefore always invalidated.
    pub fn replace_evaluator(&mut self, evaluator: E) {
        self.evaluator = evaluator;
        self.clear_transposition_table();
    }

    /// Attaches only independently verified, immutable strategy data.
    #[must_use]
    pub fn with_proof_book(mut self, book: Arc<VerifiedProofBook>) -> Self {
        self.proof_book = Some(book);
        self
    }

    pub fn set_proof_book(&mut self, book: Option<Arc<VerifiedProofBook>>) {
        self.proof_book = book;
    }

    /// Samples at most 1024 buckets regardless of configured capacity.
    #[must_use]
    pub fn transposition_table_statistics(&self) -> TranspositionTableStatistics {
        self.table.statistics()
    }

    #[must_use]
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Replaces the table with an empty table of the requested capacity.
    pub fn resize_transposition_table(&mut self, memory_mib: usize) {
        self.table = TranspositionTable::new(memory_mib);
        self.generation = 0;
        self.config = self.config.with_tt_memory_mib(memory_mib);
    }

    /// Reconfigures the engine between public searches. Changing TT capacity
    /// is performed by the engine-owning thread and clears the old table.
    pub fn reconfigure(&mut self, config: EngineConfig) {
        if config != self.config {
            self.clear_transposition_table();
        }
        if config.tt_memory_mib() != self.config.tt_memory_mib() {
            self.resize_transposition_table(config.tt_memory_mib());
        }
        if config.tactical().vct_table_memory_mib != self.config.tactical().vct_table_memory_mib {
            self.vct = VctSolver::new(config.tactical().vct_table_memory_mib);
        }
        self.config = config;
    }

    fn begin_search_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

impl Default for AlphaBetaEngine<PatternEvaluator> {
    fn default() -> Self {
        Self::new(PatternEvaluator)
    }
}

impl<E: Evaluator> SearchEngine for AlphaBetaEngine<E> {
    fn search_controlled(
        &mut self,
        position: &Position,
        limits: SearchLimits,
        cancellation: CancellationToken,
        observer: &mut dyn SearchObserver,
    ) -> SearchResult {
        let mut budget = SearchBudget::new(limits, cancellation);
        self.begin_search_generation();
        self.vcf.begin_search(self.config.vcf_max_nodes());
        self.vct.begin_search(self.config.tactical().vct.max_nodes);
        let mut state = SearchState::new(position, &self.evaluator);
        let mut statistics = SearchStatistics {
            worker_count: self.config.threads(),
            ..SearchStatistics::default()
        };
        let mut scratch = self.scratch.take().unwrap_or_default();
        scratch.reset();
        let mut result = self.search_with_budget(
            position,
            &mut state,
            limits,
            &mut budget,
            &mut statistics,
            observer,
            &mut scratch,
        );
        self.scratch = Some(scratch);
        // Final statistics include discarded partial work; score/PV/seldepth
        // still describe the last completed iteration or exact proof.
        if self.config.threads() == 1 || statistics.work_nodes == 0 {
            statistics.work_nodes = budget.work_nodes();
        }
        debug_assert_eq!(
            budget.admitted_nodes(),
            limits.max_nodes.map(|_| statistics.work_nodes)
        );
        result.statistics = statistics;
        result.termination = budget.termination();
        result
    }
}

impl<E: Evaluator> AlphaBetaEngine<E> {
    /// The application supplies its frozen engine/build identity. Model and
    /// effective profile are checked here and again at every root after changes.
    pub fn set_opening_database(
        &mut self,
        database: Arc<crate::OpeningDatabase>,
        policy: crate::OpeningPolicy,
        engine_build: &str,
    ) -> Result<(), &'static str> {
        if database.identity().engine_build != engine_build
            || database.identity().model != self.evaluator.model_fingerprint()
            || database.identity().profile
                != self
                    .config
                    .effective_profile(self.evaluator.score_contract())
        {
            return Err("opening database engine/model/profile mismatch");
        }
        self.opening_database = Some((database, policy));
        Ok(())
    }
    pub fn clear_opening_database(&mut self) {
        self.opening_database = None;
    }
    fn opening_hit(
        &self,
        position: &Position,
    ) -> Option<(crate::OpeningMove, crate::OpeningPolicy)> {
        let (db, policy) = self.opening_database.as_ref()?;
        if db.identity().model != self.evaluator.model_fingerprint()
            || db.identity().profile
                != self
                    .config
                    .effective_profile(self.evaluator.score_contract())
        {
            return None;
        }
        Some((*db.query(position, db.identity())?.moves.first()?, *policy))
    }
    #[allow(clippy::too_many_arguments)]
    fn search_with_budget(
        &mut self,
        root_position: &Position,
        state: &mut SearchState<E>,
        limits: SearchLimits,
        budget: &mut SearchBudget,
        statistics: &mut SearchStatistics,
        observer: &mut dyn SearchObserver,
        scratch: &mut WorkerScratch,
    ) -> SearchResult {
        let mut seldepth = 0;
        let pv = &mut scratch.pv;
        if let Some(score) = terminal_score(state.position(), 0) {
            // Exact facts remain usable even if admission is already stopped.
            // Never exceed the cap merely to account for a known root fact.
            statistics.nodes = u64::from(budget.charge().is_ok());
            let mut result = search_result(None, score, limits, 0, 0, Vec::new(), *statistics);
            result.origin = SearchOrigin::Terminal;
            return result;
        }
        let side = state.position().side_to_move();
        if limits.max_depth != 0
            && let Some((at, score)) =
                immediate_tactic(state.patterns(), side).resolve(0, pv, &mut seldepth)
        {
            statistics.nodes = u64::from(budget.charge().is_ok());
            statistics.work_nodes = budget.work_nodes();
            let result = search_result(
                Some(at),
                score,
                limits,
                0,
                seldepth,
                pv.root_line().to_vec(),
                *statistics,
            );
            let mut result = result;
            result.origin = SearchOrigin::Immediate;
            observer.on_info(SearchInfo::from(&result));
            return result;
        }
        // Static fallback is explicitly not a completed nominal search score.
        let fallback = (limits.max_depth != 0)
            .then(|| {
                state.candidate_bits().iter().max_by_key(|&at| {
                    resistance_key(
                        side,
                        state.patterns(),
                        at,
                        state.policy_score(&self.evaluator, at),
                    )
                })
            })
            .flatten();
        let mut completed = search_result(
            fallback,
            state.evaluate(&self.evaluator),
            limits,
            0,
            0,
            fallback.into_iter().collect(),
            *statistics,
        );
        if budget.poll().is_err() {
            return completed;
        }
        if limits.max_depth == 0 {
            completed.origin = SearchOrigin::Analysis;
            let mut resources = SearchResources {
                seldepth: &mut seldepth,
                pv,
                statistics,
                heuristics: scratch.heuristics.take().expect("exclusive worker scratch"),
                interior_proof: None,
                analysis: None,
                budget,
            };
            let outcome = self.qsearch(
                state,
                -SEARCH_INFINITY,
                SEARCH_INFINITY,
                0,
                0,
                &mut resources,
            );
            scratch.heuristics = Some(resources.heuristics);
            if let Ok(score) = outcome
                && budget.poll().is_ok()
            {
                completed.score = score;
                completed.seldepth = seldepth;
            }
            return completed;
        }
        if let Some(book) = &self.proof_book {
            statistics.proof_book_probes += 1;
            if let Some(hit) = book.query(state.position()) {
                statistics.proof_book_hits += 1;
                completed = search_result(
                    Some(hit.best_move),
                    MATE_SCORE - i32::from(hit.distance.plies()),
                    limits,
                    0,
                    0,
                    vec![hit.best_move],
                    *statistics,
                );
                completed.proof = Some(Proof {
                    source: ProofSource::ProofBook,
                    distance: hit.distance,
                });
                completed.origin = SearchOrigin::ProofBook;
                completed.statistics.work_nodes = budget.work_nodes();
                observer.on_info(SearchInfo::from(&completed));
                return completed;
            }
        }
        if let Some((hit, crate::OpeningPolicy::BookMove)) = self.opening_hit(root_position) {
            let mut result = search_result(
                Some(hit.at),
                hit.score,
                limits,
                0,
                0,
                vec![hit.at],
                *statistics,
            );
            result.origin = SearchOrigin::OpeningBook;
            observer.on_info(SearchInfo::from(&result));
            return result;
        }
        if self.config.tactical().vcf.enabled() && !forcing_moves(state.patterns(), side).is_empty()
        {
            let proof = state.prove_vcf(&mut self.vcf, side, self.config.vcf_max_plies(), budget);
            let vcf = self.vcf.statistics();
            statistics.vcf_nodes = vcf.nodes;
            statistics.vcf_cache_hits = vcf.cache_hits;
            statistics.vcf_probes = vcf.probes;
            statistics.vcf_proven = vcf.proven;
            statistics.vcf_budget_exhausted = vcf.budget_exhausted;
            if budget.poll().is_err() {
                return completed;
            }
            if let VcfStatus::ProvenWin { plies } = proof.status {
                completed = search_result(
                    proof.principal_variation.first().copied(),
                    MATE_SCORE - i32::from(plies),
                    limits,
                    0,
                    plies,
                    proof.principal_variation,
                    *statistics,
                );
                completed.proof = Some(Proof {
                    source: ProofSource::Vcf,
                    distance: ProofDistance::Exact(plies),
                });
                completed.origin = SearchOrigin::Vcf;
                completed.statistics.work_nodes = budget.work_nodes();
                observer.on_info(SearchInfo::from(&completed));
                return completed;
            }
        }
        // Poll even when VCF was gated off; these are independent root stages.
        if budget.poll().is_err() {
            return completed;
        }
        let vct_limits = self.config.tactical().vct;
        if vct_limits.enabled() && !crate::vct::attacks(state.patterns(), side).is_empty() {
            statistics.vct_probes += 1;
            let proof = state.prove_vct(&mut self.vct, side, vct_limits.max_plies, budget);
            let vct = self.vct.statistics();
            statistics.vct_nodes = vct.nodes;
            statistics.vct_cache_hits = vct.cache_hits;
            statistics.vct_proven = vct.proven;
            statistics.vct_budget_exhausted = vct.budget_exhausted;
            if budget.poll().is_err() {
                return completed;
            }
            if let VctStatus::ProvenWin { plies } = proof.status {
                completed = search_result(
                    proof.principal_variation.first().copied(),
                    MATE_SCORE - i32::from(plies),
                    limits,
                    0,
                    plies,
                    proof.principal_variation,
                    *statistics,
                );
                completed.proof = Some(Proof {
                    source: ProofSource::Vct,
                    distance: ProofDistance::Exact(plies),
                });
                completed.origin = SearchOrigin::Vct;
                completed.statistics.work_nodes = budget.work_nodes();
                observer.on_info(SearchInfo::from(&completed));
                return completed;
            }
        }
        let mut resources = SearchResources {
            seldepth: &mut seldepth,
            pv,
            statistics,
            heuristics: scratch.heuristics.take().expect("exclusive worker scratch"),
            interior_proof: None,
            analysis: scratch.analysis.take(),
            budget,
        };
        resources.interior_proof = crate::interior_proof::InteriorProof::new(self.config);
        let result = self.search_ordinary(
            root_position,
            state,
            limits,
            completed,
            &mut resources,
            observer,
        );
        scratch.heuristics = Some(resources.heuristics);
        scratch.analysis = resources.analysis;
        result
    }

    fn search_ordinary(
        &mut self,
        root_position: &Position,
        state: &mut SearchState<E>,
        limits: SearchLimits,
        completed: SearchResult,
        resources: &mut SearchResources<'_>,
        observer: &mut dyn SearchObserver,
    ) -> SearchResult {
        let threads = self.config.threads();
        let mut principal = AbContext::new(&self.evaluator, &self.table, self.generation, 0);
        principal.opening_hint = self.opening_hit(root_position).map(|(hit, _)| hit.at);
        principal.selectivity = self.config.selectivity();
        principal.root_resistance = self.config.root_resistance();
        principal.adaptive_root_candidates = self.config.adaptive_root_candidates();
        principal.profile = self
            .config
            .effective_profile(self.evaluator.score_contract());
        principal.probcut = self.config.probcut().filter(|calibration| {
            calibration.matches(
                self.evaluator.model_fingerprint(),
                principal.profile,
                principal.selectivity,
            )
        });
        if threads == 1 {
            let mut completed =
                run_principal_iterations(&principal, state, limits, resources, completed, observer);
            resources.statistics.principal_nodes = resources.statistics.nodes;
            resources.statistics.helper_nodes = 0;
            completed.statistics = *resources.statistics;
            return completed;
        }

        self.helper_scratch
            .resize_with(threads - 1, WorkerScratch::default);
        for scratch in &mut self.helper_scratch {
            scratch.reset();
        }
        let evaluator = &self.evaluator;
        let table = &self.table;
        let generation = self.generation;
        let selectivity = self.config.selectivity();
        let profile = principal.profile;
        let probcut = principal.probcut;
        let (completed, helper_results) = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(threads.saturating_sub(1));
            for (index, scratch) in self.helper_scratch.iter_mut().enumerate() {
                let worker_id = index + 1;
                let helper_budget = resources.budget.worker();
                let helper_position = root_position;
                let helper_evaluator = evaluator;
                let helper_table = table;
                handles.push(scope.spawn(move || {
                    let mut helper_budget = helper_budget;
                    let mut helper_state = SearchState::new(helper_position, helper_evaluator);
                    let mut helper_statistics = SearchStatistics::default();
                    let mut helper_seldepth = 0;
                    let mut context =
                        AbContext::new(helper_evaluator, helper_table, generation, worker_id);
                    context.selectivity = selectivity;
                    context.profile = profile;
                    context.probcut = probcut;
                    run_helper_iterations(
                        &context,
                        &mut helper_state,
                        limits,
                        &mut helper_budget,
                        &mut helper_statistics,
                        scratch,
                        &mut helper_seldepth,
                    );
                    HelperResult {
                        statistics: helper_statistics,
                        work_nodes: helper_budget.work_nodes(),
                    }
                }));
            }

            let completed =
                run_principal_iterations(&principal, state, limits, resources, completed, observer);
            // Principal completion and principal interruption both end this
            // public team. Helpers interpret this only as an internal stop.
            resources.budget.mark_team_done();
            let helper_results: Vec<HelperResult> = handles
                .drain(..)
                .map(|handle| handle.join().expect("Alpha-Beta helper panicked"))
                .collect();
            (completed, helper_results)
        });

        resources.statistics.principal_nodes = resources.statistics.nodes;
        resources.statistics.helper_nodes = 0;
        let principal_work = resources.budget.work_nodes();
        let mut helper_work = 0;
        for helper in helper_results {
            resources.statistics.helper_nodes += helper.statistics.nodes;
            helper_work += helper.work_nodes;
            resources.statistics.add_worker(helper.statistics);
        }
        resources.statistics.work_nodes = principal_work + helper_work;
        let mut completed = completed;
        completed.statistics = *resources.statistics;
        completed
    }
}

impl SearchStatistics {
    /// Schema for named diagnostic counters. Values are snapshots, not strength metrics.
    pub const COUNTER_SCHEMA: u32 = 1;

    /// Stable names shared by research tools and UI. Called only at reporting
    /// boundaries; search increments ordinary worker-local fields without atomics.
    pub fn counters(&self) -> impl Iterator<Item = (&'static str, u64)> {
        [
            ("work_nodes", self.work_nodes),
            ("nodes", self.nodes),
            ("qnodes", self.qnodes),
            ("qsearch_recursive_nodes", self.qsearch_recursive_nodes),
            ("qsearch_forcing_edges", self.qsearch_forcing_edges),
            ("qsearch_forced_blocks", self.qsearch_forced_blocks),
            ("qsearch_stand_pat_cutoffs", self.qsearch_stand_pat_cutoffs),
            ("qsearch_cap_hits", self.qsearch_cap_hits),
            ("qsearch_three_edges", self.qsearch_three_edges),
            ("qsearch_dependency_edges", self.qsearch_dependency_edges),
            ("max_qply", self.max_qply as u64),
            ("pvs_researches", self.pvs_researches),
            ("root_resistance_ties", self.root_resistance_ties),
            ("root_candidates_added", self.root_candidates_added),
            (
                "root_resistance_researches",
                self.root_resistance_researches,
            ),
            ("lmr_reductions", self.lmr_reductions),
            ("lmr_researches", self.lmr_researches),
            ("policy_lmr_reductions", self.policy_lmr_reductions),
            ("policy_lmr_researches", self.policy_lmr_researches),
            (
                "policy_lmr_failed_verifications",
                self.policy_lmr_failed_verifications,
            ),
            ("singular_attempts", self.singular_attempts),
            ("singular_extensions", self.singular_extensions),
            ("singular_incomplete", self.singular_incomplete),
            ("singular_work", self.singular_work),
            ("probcut_attempts", self.probcut_attempts),
            ("probcut_cutoffs", self.probcut_cutoffs),
            ("probcut_work", self.probcut_work),
            ("probcut_unqualified", self.probcut_unqualified),
            ("lmp_pruned_moves", self.lmp_pruned_moves),
            ("futility_pruned_moves", self.futility_pruned_moves),
            ("rfp_attempts", self.rfp_attempts),
            ("rfp_cutoffs", self.rfp_cutoffs),
            ("razor_attempts", self.razor_attempts),
            ("razor_cutoffs", self.razor_cutoffs),
            ("null_attempts", self.null_attempts),
            ("null_verifications", self.null_verifications),
            ("null_cutoffs", self.null_cutoffs),
            ("null_work", self.null_work),
            ("iid_attempts", self.iid_attempts),
            ("iid_work", self.iid_work),
            ("policy_pruned_moves", self.policy_pruned_moves),
            ("iir_reductions", self.iir_reductions),
            ("threat_extensions", self.threat_extensions),
            ("aspiration_fail_low", self.aspiration_fail_low),
            ("aspiration_fail_high", self.aspiration_fail_high),
            ("static_evaluations", self.static_evaluations),
            ("beta_cutoffs", self.beta_cutoffs),
            ("tt_probes", self.tt_probes),
            ("tt_hits", self.tt_hits),
            ("tt_cutoffs", self.tt_cutoffs),
            ("tt_stores", self.tt_stores),
            ("tt_replacements", self.tt_replacements),
            ("vcf_nodes", self.vcf_nodes),
            ("vcf_cache_hits", self.vcf_cache_hits),
            ("vcf_probes", self.vcf_probes),
            ("vcf_proven", self.vcf_proven),
            ("vcf_budget_exhausted", self.vcf_budget_exhausted),
            ("vct_nodes", self.vct_nodes),
            ("vct_probes", self.vct_probes),
            ("vct_cache_hits", self.vct_cache_hits),
            ("vct_proven", self.vct_proven),
            ("vct_budget_exhausted", self.vct_budget_exhausted),
            ("proof_book_probes", self.proof_book_probes),
            ("proof_book_hits", self.proof_book_hits),
            ("worker_count", self.worker_count as u64),
            ("principal_nodes", self.principal_nodes),
            ("helper_nodes", self.helper_nodes),
            ("interior_attempts", self.interior_proof.attempts),
            ("interior_proven", self.interior_proof.proven),
            ("interior_not_proven", self.interior_proof.not_proven),
            (
                "interior_local_exhausted",
                self.interior_proof.local_exhausted,
            ),
            ("interior_interrupted", self.interior_proof.interrupted),
            ("interior_skipped", self.interior_proof.skipped),
            ("interior_cooldown_hits", self.interior_proof.cooldown_hits),
            ("interior_work", self.interior_proof.work),
            (
                "interior_certificate_work",
                self.interior_proof.certificate_work,
            ),
            ("interior_elapsed_nanos", self.interior_proof.elapsed_nanos),
            ("interior_vct_attempts", self.interior_proof.vct_attempts),
            ("interior_vct_proven", self.interior_proof.vct_proven),
            (
                "interior_vct_not_proven",
                self.interior_proof.vct_not_proven,
            ),
            (
                "interior_vct_local_exhausted",
                self.interior_proof.vct_local_exhausted,
            ),
            (
                "interior_vct_interrupted",
                self.interior_proof.vct_interrupted,
            ),
            ("interior_vct_work", self.interior_proof.vct_work),
        ]
        .into_iter()
    }

    fn add_worker(&mut self, other: Self) {
        self.nodes += other.nodes;
        self.qnodes += other.qnodes;
        self.qsearch_recursive_nodes += other.qsearch_recursive_nodes;
        self.qsearch_forcing_edges += other.qsearch_forcing_edges;
        self.qsearch_forced_blocks += other.qsearch_forced_blocks;
        self.qsearch_stand_pat_cutoffs += other.qsearch_stand_pat_cutoffs;
        self.qsearch_cap_hits += other.qsearch_cap_hits;
        self.qsearch_three_edges += other.qsearch_three_edges;
        self.qsearch_dependency_edges += other.qsearch_dependency_edges;
        self.max_qply = self.max_qply.max(other.max_qply);
        self.pvs_researches += other.pvs_researches;
        self.root_resistance_ties += other.root_resistance_ties;
        self.root_resistance_researches += other.root_resistance_researches;
        self.root_candidates_added += other.root_candidates_added;
        self.lmr_reductions += other.lmr_reductions;
        self.lmr_researches += other.lmr_researches;
        self.policy_lmr_reductions += other.policy_lmr_reductions;
        self.policy_lmr_researches += other.policy_lmr_researches;
        self.policy_lmr_failed_verifications += other.policy_lmr_failed_verifications;
        self.singular_attempts += other.singular_attempts;
        self.singular_extensions += other.singular_extensions;
        self.singular_incomplete += other.singular_incomplete;
        self.singular_work += other.singular_work;
        self.probcut_attempts += other.probcut_attempts;
        self.probcut_cutoffs += other.probcut_cutoffs;
        self.probcut_work += other.probcut_work;
        self.probcut_unqualified += other.probcut_unqualified;
        self.lmp_pruned_moves += other.lmp_pruned_moves;
        self.futility_pruned_moves += other.futility_pruned_moves;
        self.rfp_attempts += other.rfp_attempts;
        self.rfp_cutoffs += other.rfp_cutoffs;
        self.razor_attempts += other.razor_attempts;
        self.razor_cutoffs += other.razor_cutoffs;
        self.null_attempts += other.null_attempts;
        self.null_verifications += other.null_verifications;
        self.null_cutoffs += other.null_cutoffs;
        self.null_work += other.null_work;
        self.iid_attempts += other.iid_attempts;
        self.iid_work += other.iid_work;
        self.policy_pruned_moves += other.policy_pruned_moves;
        self.iir_reductions += other.iir_reductions;
        self.threat_extensions += other.threat_extensions;
        self.aspiration_fail_low += other.aspiration_fail_low;
        self.aspiration_fail_high += other.aspiration_fail_high;
        self.static_evaluations += other.static_evaluations;
        self.beta_cutoffs += other.beta_cutoffs;
        self.tt_probes += other.tt_probes;
        self.tt_hits += other.tt_hits;
        self.tt_cutoffs += other.tt_cutoffs;
        self.tt_stores += other.tt_stores;
        self.tt_replacements += other.tt_replacements;
        self.vcf_nodes += other.vcf_nodes;
        self.vcf_cache_hits += other.vcf_cache_hits;
        self.vcf_probes += other.vcf_probes;
        self.vcf_proven += other.vcf_proven;
        self.vcf_budget_exhausted += other.vcf_budget_exhausted;
        self.vct_nodes += other.vct_nodes;
        self.vct_probes += other.vct_probes;
        self.vct_cache_hits += other.vct_cache_hits;
        self.vct_proven += other.vct_proven;
        self.vct_budget_exhausted += other.vct_budget_exhausted;
    }
}

struct HelperResult {
    statistics: SearchStatistics,
    work_nodes: u64,
}

struct PolicyRanks {
    scores: [i32; rustmoku_core::CELL_COUNT],
    len: usize,
}
impl PolicyRanks {
    fn new<E: Evaluator>(state: &SearchState<E>, evaluator: &E, moves: &MoveList) -> Self {
        let mut result = Self {
            scores: [0; rustmoku_core::CELL_COUNT],
            len: 0,
        };
        for at in moves.iter() {
            if SearchHeuristics::is_quiet(state.patterns(), state.position().side_to_move(), at)
                && let Some(score) = state.policy_score(evaluator, at)
            {
                result.scores[result.len] = score;
                result.len += 1;
            }
        }
        result.scores[..result.len].sort_unstable();
        result
    }
    fn low_tail(&self, score: Option<i32>, percent: u8) -> bool {
        score.is_some_and(|score| {
            self.len >= 10
                && self.scores[..self.len].partition_point(|&other| other <= score) * 100
                    <= self.len * usize::from(percent)
        })
    }
    fn lower_half(&self, score: Option<i32>) -> bool {
        score.is_some_and(|score| {
            self.len >= 4
                && self.len - self.scores[..self.len].partition_point(|&other| other <= score)
                    >= self.len.div_ceil(2)
        })
    }
}

fn run_principal_iterations<E: Evaluator>(
    context: &AbContext<'_, E>,
    state: &mut SearchState<E>,
    limits: SearchLimits,
    resources: &mut SearchResources<'_>,
    mut completed: SearchResult,
    observer: &mut dyn SearchObserver,
) -> SearchResult {
    resources
        .heuristics
        .set_parameters(context.profile.parameters());
    if context.profile.singular()
        || context.profile.research().iid
        || context.profile.research().null_move
        || context.probcut.is_some()
    {
        resources.analysis.get_or_insert_with(AnalysisScratch::new);
    }
    for depth in 1..=limits.max_depth {
        if resources.budget.poll().is_err() {
            break;
        }
        let Ok(iteration) =
            context.search_iteration(state, depth, completed.score, &mut *resources)
        else {
            break;
        };
        if resources.budget.poll().is_err() {
            break;
        }
        resources.statistics.work_nodes = resources.budget.work_nodes();
        resources.statistics.principal_nodes = resources.statistics.nodes;
        completed = search_result(
            iteration.best_move,
            iteration.score,
            limits,
            depth,
            *resources.seldepth,
            resources.pv.root_line().to_vec(),
            *resources.statistics,
        );
        completed.origin = SearchOrigin::AlphaBeta;
        observer.on_info(SearchInfo::from(&completed));
        if observer.should_stop() {
            break;
        }
    }
    completed
}

fn run_helper_iterations<E: Evaluator>(
    context: &AbContext<'_, E>,
    state: &mut SearchState<E>,
    limits: SearchLimits,
    budget: &mut SearchBudget,
    statistics: &mut SearchStatistics,
    scratch: &mut WorkerScratch,
    seldepth: &mut u8,
) {
    let mut previous_score = state.evaluate(context.evaluator);
    let mut resources = SearchResources {
        seldepth,
        pv: &mut scratch.pv,
        statistics,
        heuristics: scratch.heuristics.take().expect("exclusive helper scratch"),
        interior_proof: None,
        analysis: scratch.analysis.take(),
        budget,
    };
    resources
        .heuristics
        .set_parameters(context.profile.parameters());
    if context.profile.singular()
        || context.profile.research().iid
        || context.profile.research().null_move
        || context.probcut.is_some()
    {
        resources.analysis.get_or_insert_with(AnalysisScratch::new);
    }
    for depth in 1..=limits.max_depth {
        if resources.budget.poll().is_err() {
            break;
        }
        let Ok(iteration) = context.search_iteration(state, depth, previous_score, &mut resources)
        else {
            break;
        };
        if resources.budget.poll().is_err() {
            break;
        }
        previous_score = iteration.score;
        resources.statistics.work_nodes = resources.budget.work_nodes();
    }
    scratch.heuristics = Some(resources.heuristics);
    scratch.analysis = resources.analysis;
}

/// Borrowed immutable engine components plus worker-specific deterministic
/// root diversity. Every mutable search structure remains in the call's
/// `SearchResources` and `SearchState`.
struct AbContext<'a, E: Evaluator> {
    evaluator: &'a E,
    table: &'a TranspositionTable,
    generation: u8,
    root_rotation: usize,
    opening_hint: Option<Move>,
    selectivity: crate::SelectivityConfig,
    root_resistance: bool,
    adaptive_root_candidates: bool,
    domain: SearchDomain,
    profile: crate::SearchProfile,
    probcut: Option<crate::ProbCutCalibration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchDomain {
    /// Hypothetical pass subtree: no ordinary TT, proofs, or recursive experiments.
    Null,
    Normal,
    Analysis,
    /// All legal nominal descendants. Quiescence remains an explicit leaf policy.
    Teacher,
    Excluded {
        at: Move,
        ply: u8,
    },
}

impl<'a, E: Evaluator> AbContext<'a, E> {
    fn candidates(&self, state: &SearchState<E>) -> MoveList {
        if self.domain == SearchDomain::Teacher {
            let mut moves = MoveList::new();
            for at in crate::TeacherCandidateUniverse::moves(state.position()) {
                moves.push(at);
            }
            moves
        } else {
            state.candidates()
        }
    }
    fn new(
        evaluator: &'a E,
        table: &'a TranspositionTable,
        generation: u8,
        root_rotation: usize,
    ) -> Self {
        Self {
            evaluator,
            table,
            generation,
            root_rotation,
            opening_hint: None,
            selectivity: crate::SelectivityConfig::BASELINE,
            root_resistance: false,
            adaptive_root_candidates: false,
            domain: SearchDomain::Normal,
            profile: crate::SearchProfile::baseline(evaluator.score_contract()),
            probcut: None,
        }
    }

    fn probe_tt(
        &self,
        state: &SearchState<E>,
        depth: u8,
        alpha: i32,
        beta: i32,
        ply: u8,
        statistics: &mut SearchStatistics,
    ) -> TtProbe {
        if self.domain != SearchDomain::Normal {
            return TtProbe::default();
        }
        statistics.tt_probes += 1;
        let Some(entry) = self.table.probe(state.key().value()) else {
            return TtProbe::default();
        };
        statistics.tt_hits += 1;

        let best_move = entry
            .best_move()
            .filter(|&at| state.position().is_legal(at));
        let reuse_depth = if self.profile.research().competitive_tt && entry.depth >= depth {
            entry.depth
        } else {
            depth
        };
        let cutoff_score = tt_cutoff_score(entry, reuse_depth, alpha, beta, ply);
        if cutoff_score.is_some() {
            statistics.tt_cutoffs += 1;
        }
        TtProbe {
            best_move,
            cutoff_score,
            entry: Some(entry),
        }
    }

    fn store_tt(&self, store: TtStore, statistics: &mut SearchStatistics) {
        if self.domain != SearchDomain::Normal {
            return;
        }
        let entry = TtEntry::new(
            store.key,
            score_to_tt(store.score, store.ply),
            store.best_move,
            store.depth,
            store.bound,
            self.generation,
        );
        let outcome = self.table.store_with_outcome(entry);
        if outcome.stored {
            statistics.tt_stores += 1;
        }
        statistics.tt_replacements += u64::from(outcome.replacement);
    }
}

// Keep the small private test/oracle surface attached to AlphaBetaEngine while
// production workers use AbContext directly and borrow only Sync components.
impl<E: Evaluator> AlphaBetaEngine<E> {
    pub fn effective_search_profile(&self) -> crate::SearchProfile {
        self.config
            .effective_profile(self.evaluator.score_contract())
    }
    fn ab_context(&self) -> AbContext<'_, E> {
        AbContext::new(&self.evaluator, &self.table, self.generation, 0)
    }

    #[cfg(test)]
    fn search_iteration(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        previous_score: i32,
        resources: &mut SearchResources<'_>,
    ) -> Result<RootSearchResult, Stopped> {
        self.ab_context()
            .search_iteration(state, depth, previous_score, resources)
    }

    #[cfg(test)]
    fn search_root<const PVS: bool>(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        alpha: i32,
        beta: i32,
        resources: &mut SearchResources<'_>,
    ) -> Result<RootSearchResult, Stopped> {
        self.ab_context()
            .search_root::<PVS>(state, depth, alpha, beta, resources)
    }

    #[cfg(test)]
    fn negamax<const PVS: bool>(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        alpha: i32,
        beta: i32,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<NodeResult, Stopped> {
        self.ab_context()
            .negamax::<PVS>(state, depth, alpha, beta, ply, resources)
    }

    fn qsearch(
        &self,
        state: &mut SearchState<E>,
        alpha: i32,
        beta: i32,
        ply: u8,
        qply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<i32, Stopped> {
        self.ab_context()
            .qsearch(state, alpha, beta, ply, qply, resources)
    }

    #[cfg(test)]
    fn probe_tt(
        &self,
        state: &SearchState<E>,
        depth: u8,
        alpha: i32,
        beta: i32,
        ply: u8,
        statistics: &mut SearchStatistics,
    ) -> TtProbe {
        self.ab_context()
            .probe_tt(state, depth, alpha, beta, ply, statistics)
    }
}

struct SearchResources<'a> {
    seldepth: &'a mut u8,
    pv: &'a mut PvTable,
    statistics: &'a mut SearchStatistics,
    heuristics: SearchHeuristics,
    interior_proof: Option<crate::interior_proof::InteriorProof>,
    analysis: Option<Box<AnalysisScratch>>,
    budget: &'a mut SearchBudget,
}

struct WorkerScratch {
    pv: PvTable,
    heuristics: Option<SearchHeuristics>,
    analysis: Option<Box<AnalysisScratch>>,
}
impl Default for WorkerScratch {
    fn default() -> Self {
        Self {
            pv: PvTable::new(),
            heuristics: Some(SearchHeuristics::default()),
            analysis: None,
        }
    }
}
impl WorkerScratch {
    fn reset(&mut self) {
        self.pv.reset();
        self.heuristics
            .as_mut()
            .expect("returned worker scratch")
            .reset();
        if let Some(analysis) = &mut self.analysis {
            analysis.pv.reset();
            analysis.seldepth = 0;
            analysis
                .heuristics
                .as_mut()
                .expect("returned analysis scratch")
                .reset();
        }
    }
}

struct AnalysisScratch {
    pv: PvTable,
    heuristics: Option<SearchHeuristics>,
    seldepth: u8,
}
impl AnalysisScratch {
    fn new() -> Box<Self> {
        Box::new(Self {
            pv: PvTable::new(),
            heuristics: Some(SearchHeuristics::default()),
            seldepth: 0,
        })
    }
}

#[derive(Clone, Copy)]
struct TtStore {
    key: u64,
    score: i32,
    best_move: Option<Move>,
    depth: u8,
    bound: Bound,
    ply: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TtProbe {
    best_move: Option<Move>,
    cutoff_score: Option<i32>,
    entry: Option<TtEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RootSearchResult {
    best_move: Option<Move>,
    score: i32,
}

fn tt_cutoff_score(entry: TtEntry, depth: u8, alpha: i32, beta: i32, ply: u8) -> Option<i32> {
    // A deeper heuristic score has a different horizon and is not a bound on
    // this fixed-depth minimax value. Exact depth preserves cold/warm semantics
    // across arbitrary public-search history; deeper legal moves still order.
    if entry.depth != depth {
        return None;
    }
    let score = score_from_tt(entry.score, ply);
    match entry.bound {
        Bound::Exact => Some(score),
        Bound::Lower if score >= beta => Some(score),
        Bound::Upper if score <= alpha => Some(score),
        Bound::Empty | Bound::Lower | Bound::Upper => None,
    }
}

const fn classify_bound(best_score: i32, original_alpha: i32, beta: i32) -> Bound {
    if best_score <= original_alpha {
        Bound::Upper
    } else if best_score >= beta {
        Bound::Lower
    } else {
        Bound::Exact
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MateDistanceWindow {
    Search { alpha: i32, beta: i32 },
    Cutoff(i32),
}

fn mate_distance_window(alpha: i32, beta: i32, ply: u8) -> MateDistanceWindow {
    let alpha = alpha.max(-MATE_SCORE + i32::from(ply));
    if alpha >= beta {
        return MateDistanceWindow::Cutoff(alpha);
    }
    let beta = beta.min(MATE_SCORE - i32::from(ply) - 1);
    if alpha >= beta {
        MateDistanceWindow::Cutoff(beta)
    } else {
        MateDistanceWindow::Search { alpha, beta }
    }
}

fn threat_extension(profile: ThreatProfile, used: u8) -> bool {
    profile >= ThreatProfile::FourThree && used < search_params::THREAT_EXTENSION_BUDGET
}

fn terminal_score(position: &Position, ply: u8) -> Option<i32> {
    if let Some(winner) = position.winner() {
        let distance = i32::from(ply);
        return Some(if winner == position.side_to_move() {
            MATE_SCORE - distance
        } else {
            -MATE_SCORE + distance
        });
    }
    position.is_full().then_some(0)
}

fn search_result(
    best_move: Option<Move>,
    score: i32,
    limits: SearchLimits,
    completed_depth: u8,
    seldepth: u8,
    principal_variation: Vec<Move>,
    statistics: SearchStatistics,
) -> SearchResult {
    SearchResult {
        best_move,
        score,
        requested_depth: limits.max_depth,
        proof: None,
        origin: SearchOrigin::Fallback,
        termination: SearchTermination::Completed,
        completed_depth,
        seldepth,
        principal_variation,
        statistics,
    }
}

#[cfg(test)]
mod tests {
    use rustmoku_core::{Move, Position};

    use super::{
        AlphaBetaEngine, MateDistanceWindow, SearchEngine, SearchLimits, SearchStatistics,
        classify_bound, mate_distance_window, threat_extension, tt_cutoff_score,
    };
    use crate::transposition_table::{Bound, TtEntry};
    use crate::{EngineConfig, Evaluator, search_state::SearchState, zobrist::PositionKey};

    fn entry(depth: u8, score: i32, bound: Bound) -> TtEntry {
        TtEntry::new(7, score, Some(Move::CENTER), depth, bound, 1)
    }

    struct ZeroEvaluator;

    impl Evaluator for ZeroEvaluator {
        type State = ();
        type Undo = ();
        fn initialize(&self, _position: &Position, _patterns: &crate::PatternState) {}
        fn make_move(&self, _state: &mut (), _delta: &crate::PatternDelta) {}
        fn unmake_move(&self, _state: &mut (), _delta: &crate::PatternDelta, _undo: ()) {}
        fn evaluate(
            &self,
            _position: &Position,
            _patterns: &crate::PatternState,
            _state: &(),
        ) -> i32 {
            0
        }
    }

    #[test]
    fn exact_entry_returns_at_sufficient_depth() {
        assert_eq!(
            tt_cutoff_score(entry(4, 25, Bound::Exact), 4, -10, 10, 0),
            Some(25)
        );
    }

    #[test]
    fn lower_entry_cuts_off_only_at_beta() {
        assert_eq!(
            tt_cutoff_score(entry(4, 25, Bound::Lower), 4, -10, 20, 0),
            Some(25)
        );
        assert_eq!(
            tt_cutoff_score(entry(4, 15, Bound::Lower), 4, -10, 20, 0),
            None
        );
    }

    #[test]
    fn upper_entry_cuts_off_only_at_alpha() {
        assert_eq!(
            tt_cutoff_score(entry(4, -25, Bound::Upper), 4, -20, 20, 0),
            Some(-25)
        );
        assert_eq!(
            tt_cutoff_score(entry(4, -15, Bound::Upper), 4, -20, 20, 0),
            None
        );
    }

    #[test]
    fn insufficient_depth_never_returns_score_but_retains_move() {
        let entry = entry(3, 25, Bound::Exact);
        assert_eq!(tt_cutoff_score(entry, 4, -10, 10, 0), None);
        assert_eq!(entry.best_move(), Some(Move::CENTER));
    }

    #[test]
    fn deeper_horizon_is_not_an_exact_score_or_bound_for_shallower_search() {
        for bound in [Bound::Exact, Bound::Lower, Bound::Upper] {
            for score in [-25, 25] {
                assert_eq!(tt_cutoff_score(entry(5, score, bound), 4, -10, 10, 0), None);
            }
        }
    }

    #[test]
    fn mate_distance_window_matches_terminal_distance_convention() {
        assert_eq!(
            mate_distance_window(
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
                5,
            ),
            MateDistanceWindow::Search {
                alpha: -crate::score::MATE_SCORE + 5,
                beta: crate::score::MATE_SCORE - 6,
            }
        );
        assert_eq!(
            mate_distance_window(
                -crate::score::SEARCH_INFINITY,
                -crate::score::MATE_SCORE + 4,
                5
            ),
            MateDistanceWindow::Cutoff(-crate::score::MATE_SCORE + 5)
        );
        assert_eq!(
            mate_distance_window(
                crate::score::MATE_SCORE - 5,
                crate::score::SEARCH_INFINITY,
                5
            ),
            MateDistanceWindow::Cutoff(crate::score::MATE_SCORE - 6)
        );
    }

    #[test]
    fn threat_extension_is_strong_and_path_bounded() {
        use crate::pattern::ThreatProfile;
        assert!(!threat_extension(ThreatProfile::Four, 0));
        assert!(threat_extension(ThreatProfile::FourThree, 0));
        assert!(threat_extension(ThreatProfile::OpenFour, 0));
        assert!(!threat_extension(ThreatProfile::OpenFour, 1));
    }

    #[test]
    fn insufficient_depth_probe_still_supplies_legal_hash_move() {
        let mut position = Position::default();
        position
            .make_move(Move::CENTER)
            .expect("center must be legal");
        let state = SearchState::new(&position, &ZeroEvaluator);
        let hash_move = Move::from_row_col(5, 5).expect("test move must be valid");
        let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(0));
        engine.generation = 1;
        engine.table.store(TtEntry::new(
            state.key().value(),
            0,
            Some(hash_move),
            1,
            Bound::Exact,
            1,
        ));
        let mut statistics = SearchStatistics::default();

        let probe = engine.probe_tt(&state, 2, -10, 10, 0, &mut statistics);

        assert_eq!(probe.best_move, Some(hash_move));
        assert_eq!(probe.cutoff_score, None);
        assert_eq!(statistics.tt_hits, 1);
    }

    #[test]
    fn root_equal_score_choice_ignores_injected_tt_ordering() {
        let mut position = Position::default();
        position
            .make_move(Move::CENTER)
            .expect("center must be legal");
        let noncanonical = Move::from_row_col(9, 9).expect("test move must be valid");
        let canonical = Move::from_row_col(5, 5).expect("test move must be valid");
        let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(0));
        engine.table.store(TtEntry::new(
            PositionKey::from_position(&position).value(),
            0,
            Some(noncanonical),
            1,
            Bound::Exact,
            0,
        ));

        let result = engine.search(&position, SearchLimits::new(1));

        assert_eq!(result.best_move, Some(canonical));
    }

    #[test]
    fn one_public_iterative_search_uses_one_generation() {
        let position = Position::default();
        let key = PositionKey::from_position(&position).value();
        let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(0));

        engine.search(&position, SearchLimits::new(3));
        assert_eq!(engine.generation, 1);
        assert_eq!(
            engine.table.probe(key).map(|entry| entry.generation),
            Some(1)
        );

        engine.search(&position, SearchLimits::new(3));
        assert_eq!(engine.generation, 2);
        assert_eq!(
            engine.table.probe(key).map(|entry| entry.generation),
            Some(2)
        );
    }

    #[test]
    fn stored_bound_uses_original_window() {
        assert_eq!(classify_bound(-10, -10, 20), Bound::Upper);
        assert_eq!(classify_bound(20, -10, 20), Bound::Lower);
        assert_eq!(classify_bound(5, -10, 20), Bound::Exact);
    }

    #[test]
    fn public_generation_rollover_preserves_entries_and_explicit_clear_still_works() {
        let position = Position::default();
        let key = PositionKey::from_position(&position).value();
        let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(0));
        let stored = TtEntry::new(key, 42, Some(Move::CENTER), 5, Bound::Exact, 255);
        engine.table.store(stored);
        engine.generation = 255;
        for _ in 0..260 {
            engine.search(&position, SearchLimits::new(0));
        }
        assert_eq!(engine.generation, 3);
        assert_eq!(engine.table.probe(key), Some(stored));
        assert_eq!(tt_cutoff_score(stored, 5, -100, 100, 0), Some(42));
        assert_eq!(tt_cutoff_score(stored, 6, -100, 100, 0), None);
        engine.clear_transposition_table();
        assert!(engine.table.probe(key).is_none());
        engine.table.store(stored);
        engine.resize_transposition_table(1);
        assert!(engine.table.probe(key).is_none());
        assert_eq!(
            engine.transposition_table_statistics().capacity_bytes,
            1024 * 1024
        );
    }

    #[test]
    fn resizing_transposition_table_keeps_public_configuration_coherent() {
        let mut engine =
            AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(1).with_threads(3));
        engine.resize_transposition_table(2);
        assert_eq!(engine.config().tt_memory_mib(), 2);
        assert_eq!(engine.config().threads(), 3);
        assert_eq!(
            engine.transposition_table_statistics().capacity_bytes,
            2 * 1024 * 1024
        );
        engine.reconfigure(EngineConfig::new(4).with_threads(5));
        assert_eq!(engine.config().tt_memory_mib(), 4);
        assert_eq!(engine.config().threads(), 5);
    }

    #[test]
    fn actual_search_recursion_restores_all_incremental_state() {
        use crate::{PatternEvaluator, principal_variation::PvTable};
        let mut position = Position::default();
        for index in [112, 97, 128, 113, 127, 98] {
            position
                .make_move(Move::from_index(index).unwrap())
                .unwrap();
        }
        let mut state = SearchState::new(&position, &PatternEvaluator);
        let engine = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1));
        let mut statistics = SearchStatistics::default();
        let mut pv = PvTable::new();
        let mut seldepth = 0;
        let mut resources = super::SearchResources {
            interior_proof: None,
            analysis: None,
            budget: &mut crate::search_control::SearchBudget::default(),
            statistics: &mut statistics,
            pv: &mut pv,
            seldepth: &mut seldepth,
            heuristics: crate::search_heuristics::SearchHeuristics::default(),
        };
        for depth in 1..=3 {
            engine
                .search_root::<true>(
                    &mut state,
                    depth,
                    -crate::score::SEARCH_INFINITY,
                    crate::score::SEARCH_INFINITY,
                    &mut resources,
                )
                .unwrap();
            state.assert_consistent(&PatternEvaluator);
            assert_eq!(state.position(), &position);
        }
    }

    #[test]
    fn occupied_tt_move_is_not_used_for_ordering() {
        let mut position = Position::default();
        position.make_move(Move::CENTER).unwrap();
        let state = SearchState::new(&position, &ZeroEvaluator);
        let engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(0));
        engine.table.store(TtEntry::new(
            state.key().value(),
            25,
            Some(Move::CENTER),
            0,
            Bound::Exact,
            1,
        ));
        let probe = engine.probe_tt(&state, 1, -100, 100, 0, &mut SearchStatistics::default());
        assert_eq!(probe.best_move, None);
        assert_eq!(probe.cutoff_score, None);
    }

    fn fixture(indices: &[usize]) -> Position {
        let mut position = Position::default();
        for &index in indices {
            position
                .make_move(Move::from_index(index).unwrap())
                .unwrap();
        }
        position
    }

    #[derive(Clone, Copy)]
    struct FixedEvaluator(i32);

    impl Evaluator for FixedEvaluator {
        type State = ();
        type Undo = ();
        fn initialize(&self, _: &Position, _: &crate::PatternState) {}
        fn make_move(&self, _: &mut (), _: &crate::PatternDelta) {}
        fn unmake_move(&self, _: &mut (), _: &crate::PatternDelta, _: ()) {}
        fn evaluate(&self, _: &Position, _: &crate::PatternState, _: &()) -> i32 {
            self.0
        }
    }

    fn selective_probe(score: i32) -> (super::NodeResult, SearchStatistics, bool) {
        let evaluator = FixedEvaluator(score);
        let position = fixture(&[112]);
        let mut state = SearchState::new(&position, &evaluator);
        let engine = AlphaBetaEngine::with_config(evaluator, EngineConfig::new(1));
        let mut statistics = SearchStatistics::default();
        let mut pv = crate::principal_variation::PvTable::new();
        let mut seldepth = 0;
        let result = engine
            .negamax::<true>(
                &mut state,
                2,
                0,
                1,
                0,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap();
        state.assert_consistent(&evaluator);
        let stored = engine.table.probe(state.key().value()).is_some();
        (result, statistics, stored)
    }

    #[test]
    fn direct_rfp_and_razor_results_cannot_publish_tt_bounds() {
        let (rfp, rfp_stats, rfp_stored) = selective_probe(100_000);
        assert_eq!((rfp_stats.rfp_attempts, rfp_stats.rfp_cutoffs), (1, 1));
        assert_eq!(rfp.validity, super::BoundValidity::UNVERIFIED);
        assert!(!rfp_stored);

        let (razor, razor_stats, razor_stored) = selective_probe(-100_000);
        assert_eq!(
            (razor_stats.razor_attempts, razor_stats.razor_cutoffs),
            (1, 1)
        );
        assert_eq!(razor.validity, super::BoundValidity::UNVERIFIED);
        assert!(!razor_stored);
    }

    #[test]
    fn iir_uses_actual_depth_restores_state_and_mismatched_tt_move_suppresses_it() {
        let position = fixture(&[112]);
        let key = PositionKey::from_position(&position).value();

        let engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(1));
        let mut state = SearchState::new(&position, &ZeroEvaluator);
        let mut statistics = SearchStatistics::default();
        let mut pv = crate::principal_variation::PvTable::new();
        let mut seldepth = 0;
        let result = engine
            .negamax::<true>(
                &mut state,
                crate::search_params::IIR_MIN_DEPTH,
                0,
                1,
                0,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap();
        assert!(statistics.iir_reductions > 0);
        assert_eq!(result.validity, super::BoundValidity::UNVERIFIED);
        assert!(
            engine
                .table
                .probe(key)
                .is_none_or(|entry| { entry.depth < crate::search_params::IIR_MIN_DEPTH })
        );
        assert_eq!(state.position(), &position);
        state.assert_consistent(&ZeroEvaluator);

        let guided = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(1));
        let tt_move = Move::from_row_col(5, 5).unwrap();
        guided
            .table
            .store(TtEntry::new(key, 0, Some(tt_move), 1, Bound::Exact, 1));
        let mut guided_state = SearchState::new(&position, &ZeroEvaluator);
        let mut guided_statistics = SearchStatistics::default();
        let probe = guided.probe_tt(
            &guided_state,
            crate::search_params::IIR_MIN_DEPTH,
            0,
            1,
            0,
            &mut guided_statistics,
        );
        assert_eq!(probe.best_move, Some(tt_move));
        assert_eq!(probe.cutoff_score, None);
        let mut guided_pv = crate::principal_variation::PvTable::new();
        let mut guided_seldepth = 0;
        let _guided_result = guided
            .negamax::<true>(
                &mut guided_state,
                crate::search_params::IIR_MIN_DEPTH,
                0,
                1,
                0,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut guided_seldepth,
                    pv: &mut guided_pv,
                    statistics: &mut guided_statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap();
        assert_eq!(guided_statistics.iir_reductions, 0);
        assert_eq!(guided_state.position(), &position);
        guided_state.assert_consistent(&ZeroEvaluator);
    }

    fn fixed_search<const PVS: bool>(position: &Position, depth: u8) -> super::RootSearchResult {
        let engine = AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
        let mut state = SearchState::new(position, &crate::PatternEvaluator);
        let mut statistics = SearchStatistics::default();
        let mut pv = crate::principal_variation::PvTable::new();
        let mut seldepth = 0;
        engine
            .search_root::<PVS>(
                &mut state,
                depth,
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap()
    }

    #[test]
    fn pvs_and_aspiration_match_full_window_alpha_beta() {
        for (indices, depth) in [
            (&[][..], 2),
            (&[112][..], 2),
            (&[112, 97, 128, 113][..], 3),
            (&[109, 0, 110, 2, 112, 4, 113, 6][..], 2),
        ] {
            let position = fixture(indices);
            let reference = fixed_search::<false>(&position, depth);
            assert_eq!(fixed_search::<true>(&position, depth), reference);
            let mut engine =
                AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
            for _ in 0..2 {
                let result = engine.search(&position, SearchLimits::new(depth));
                assert_eq!(
                    (result.best_move, result.score),
                    (reference.best_move, reference.score)
                );
                assert_eq!(
                    result.principal_variation.first().copied(),
                    result.best_move
                );
                let mut replay = position.clone();
                for at in result.principal_variation {
                    replay.make_move(at).expect("legal re-search PV");
                }
            }
            if indices.len() == 8 {
                assert_eq!(reference.score, crate::score::MATE_SCORE - 1);
            }
        }
    }

    #[test]
    fn aspiration_recovers_from_fail_low_and_fail_high() {
        let position = fixture(&[112, 97, 128, 113]);
        let reference = fixed_search::<false>(&position, 2);
        for offset in [-100_000, 100_000] {
            let engine =
                AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
            let mut state = SearchState::new(&position, &crate::PatternEvaluator);
            let mut statistics = SearchStatistics::default();
            let mut pv = crate::principal_variation::PvTable::new();
            let mut seldepth = 0;
            let result = engine
                .search_iteration(
                    &mut state,
                    2,
                    reference.score + offset,
                    &mut super::SearchResources {
                        interior_proof: None,
                        analysis: None,
                        budget: &mut crate::search_control::SearchBudget::default(),
                        seldepth: &mut seldepth,
                        pv: &mut pv,
                        statistics: &mut statistics,
                        heuristics: crate::search_heuristics::SearchHeuristics::default(),
                    },
                )
                .unwrap();
            assert_eq!(result, reference);
            if offset < 0 {
                assert!(statistics.aspiration_fail_high > 0);
            } else {
                assert!(statistics.aspiration_fail_low > 0);
            }
            assert_eq!(
                engine.table.probe(state.key().value()).unwrap().bound,
                Bound::Exact
            );
        }
    }

    #[test]
    fn canonical_lower_root_bound_is_not_mistaken_for_an_exact_tie() {
        struct PenaltyEvaluator;
        impl Evaluator for PenaltyEvaluator {
            type State = ();
            type Undo = ();
            fn initialize(&self, _: &Position, _: &crate::PatternState) {}
            fn make_move(&self, _: &mut (), _: &crate::PatternDelta) {}
            fn unmake_move(&self, _: &mut (), _: &crate::PatternDelta, _: ()) {}
            fn evaluate(&self, position: &Position, _: &crate::PatternState, _: &()) -> i32 {
                -i32::from(
                    position.cell(Move::from_index(80).unwrap())
                        == Some(rustmoku_core::Stone::White),
                )
            }
        }
        let position = fixture(&[112]);
        let engine = AlphaBetaEngine::with_config(PenaltyEvaluator, EngineConfig::new(1));
        engine.table.store(TtEntry::new(
            PositionKey::from_position(&position).value(),
            0,
            Some(Move::from_index(144).unwrap()),
            2,
            Bound::Exact,
            0,
        ));
        let child = fixture(&[112, 80]);
        // The child value is +1, but the valid lower bound 0 makes its scout
        // fail high at beta=0. Negation looks like a root tie until re-searched.
        engine.table.store(TtEntry::new(
            PositionKey::from_position(&child).value(),
            0,
            None,
            1,
            Bound::Lower,
            0,
        ));
        for _ in 0..2 {
            let mut state = SearchState::new(&position, &PenaltyEvaluator);
            let mut statistics = SearchStatistics::default();
            let mut pv = crate::principal_variation::PvTable::new();
            let mut seldepth = 0;
            let result = engine
                .search_root::<true>(
                    &mut state,
                    2,
                    -crate::score::SEARCH_INFINITY,
                    crate::score::SEARCH_INFINITY,
                    &mut super::SearchResources {
                        interior_proof: None,
                        analysis: None,
                        budget: &mut crate::search_control::SearchBudget::default(),
                        seldepth: &mut seldepth,
                        pv: &mut pv,
                        statistics: &mut statistics,
                        heuristics: crate::search_heuristics::SearchHeuristics::default(),
                    },
                )
                .unwrap();
            assert_eq!(result.best_move, Some(Move::from_index(81).unwrap()));
            assert_eq!(result.score, 0);
        }
    }

    fn q_result(position: &Position, qply: u8) -> (i32, Vec<Move>, SearchStatistics, u8) {
        let engine = AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
        let mut state = SearchState::new(position, &crate::PatternEvaluator);
        let mut statistics = SearchStatistics::default();
        let mut pv = crate::principal_variation::PvTable::new();
        let mut seldepth = 0;
        let score = engine
            .qsearch(
                &mut state,
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
                0,
                qply,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap();
        state.assert_consistent(&crate::PatternEvaluator);
        assert_eq!(state.position(), position);
        assert_eq!(statistics.tt_probes + statistics.tt_stores, 0);
        let mut replay = position.clone();
        for &at in pv.root_line() {
            replay.make_move(at).expect("legal qsearch PV");
        }
        (score, pv.root_line().to_vec(), statistics, seldepth)
    }

    #[test]
    fn horizon_immediate_win_and_forced_block() {
        let win = fixture(&[109, 0, 110, 2, 112, 4, 113, 6]);
        let (score, pv, _, _) = q_result(&win, 0);
        assert_eq!(score, crate::score::MATE_SCORE - 1);
        assert_eq!(pv, [Move::from_index(111).unwrap()]);
        let block = fixture(&[107, 108, 0, 109, 2, 110, 15, 111]);
        let (score, pv, _, seldepth) = q_result(&block, 0);
        assert_eq!(pv.first(), Some(&Move::CENTER));
        assert!(score > -crate::score::MATE_THRESHOLD);
        assert!(seldepth <= super::MAX_QSEARCH_PLY);
    }

    #[test]
    fn forcing_four_continues_beyond_nominal_horizon() {
        let position = fixture(&[110, 0, 111, 2, 112, 15]);
        let (score, pv, stats, seldepth) = q_result(&position, 0);
        assert_eq!(score, crate::score::MATE_SCORE - 3);
        assert_eq!(pv.len(), 3);
        assert!(stats.qnodes >= 2);
        assert_eq!(stats.qsearch_recursive_nodes, stats.qnodes - 1);
        assert!(stats.qsearch_forcing_edges > 0);
        assert!(stats.max_qply > 0);
        assert!((3..=super::MAX_QSEARCH_PLY).contains(&seldepth));
        // The explicit cap applies even to a forcing position.
        let (_, capped_pv, capped, capped_depth) = q_result(&position, super::MAX_QSEARCH_PLY);
        assert!(capped_pv.is_empty());
        assert_eq!(
            (capped.qnodes, capped.static_evaluations, capped_depth),
            (1, 1, 0)
        );
    }

    #[test]
    fn qsearch_stops_without_searching_quiet_candidates() {
        let position = fixture(&[112]);
        let (score, pv, stats, seldepth) = q_result(&position, 0);
        let patterns = crate::PatternState::new(&position);
        assert_eq!(
            score,
            crate::PatternEvaluator.evaluate(&position, &patterns, &())
        );
        assert!(pv.is_empty());
        assert_eq!(
            (
                stats.nodes,
                stats.qnodes,
                stats.static_evaluations,
                seldepth
            ),
            (1, 1, 1, 0)
        );
    }

    #[test]
    fn qsearch_cap_cannot_hide_an_immediate_win() {
        let position = fixture(&[109, 0, 110, 2, 112, 4, 113, 6]);
        let (score, pv, stats, seldepth) = q_result(&position, super::MAX_QSEARCH_PLY);
        assert_eq!(score, crate::score::MATE_SCORE - 1);
        assert_eq!(pv, [Move::from_index(111).unwrap()]);
        assert_eq!(
            (stats.qnodes, stats.static_evaluations, seldepth),
            (1, 0, 1)
        );
    }

    #[test]
    fn single_immediate_threat_restricts_normal_and_capped_qsearch_to_block() {
        let position = fixture(&[107, 108, 0, 109, 2, 110, 15, 111]);
        let mut engine =
            AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
        let result = engine.search(&position, SearchLimits::new(1));
        assert_eq!(result.best_move, Some(Move::CENTER));
        assert_eq!((result.statistics.nodes, result.statistics.qnodes), (2, 1));
        let (score, pv, stats, _) = q_result(&position, super::MAX_QSEARCH_PLY);
        assert_eq!(pv, [Move::CENTER]);
        assert_eq!(score, result.score);
        assert_eq!(stats.qnodes, 2);
        assert_eq!(stats.qsearch_forced_blocks, 1);
        let patterns = crate::PatternState::new(&position);
        assert_eq!(
            crate::tactical::immediate_tactic(&patterns, position.side_to_move()),
            crate::tactical::ImmediateTactic::ForcedBlock(Move::CENTER)
        );
        assert_eq!(
            crate::search_heuristics::SearchHeuristics::default().lmr_reduction(
                6,
                20,
                position.side_to_move(),
                Move::CENTER,
                0,
                &patterns
            ),
            0
        );
    }

    #[test]
    fn double_immediate_threat_is_exact_loss_with_legal_canonical_pv() {
        let position = fixture(&[0, 108, 2, 109, 15, 110, 17, 111]);
        let (score, pv, stats, seldepth) = q_result(&position, super::MAX_QSEARCH_PLY);
        assert_eq!(score, -crate::score::MATE_SCORE + 2);
        assert_eq!(
            (stats.qnodes, stats.static_evaluations, seldepth),
            (1, 0, 2)
        );
        assert_eq!(pv.len(), 2);
        assert_eq!(pv[0], Move::from_index(107).unwrap());
        assert_eq!(pv[1], Move::from_index(112).unwrap());
        assert!(position.is_legal(Move::from_index(1).unwrap()));
        assert!(position.would_win(pv[0], position.side_to_move().opponent()));
        let mut replay = position.clone();
        replay.make_move(pv[0]).unwrap();
        assert!(replay.would_win(pv[1], replay.side_to_move()));
        replay.make_move(pv[1]).unwrap();
        assert_eq!(replay.winner(), Some(position.side_to_move().opponent()));
        let mut engine =
            AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
        // An untrusted cached move/score must not override the board proof.
        engine.table.store(TtEntry::new(
            PositionKey::from_position(&position).value(),
            1234,
            Some(Move::CENTER),
            4,
            Bound::Exact,
            0,
        ));
        let result = engine.search(&position, SearchLimits::new(4));
        assert_eq!((result.best_move, result.score), (Some(pv[0]), score));
        assert_eq!(result.principal_variation, pv);
        assert_eq!(engine.search(&position, SearchLimits::new(4)), result);
        assert_eq!((result.statistics.nodes, result.statistics.qnodes), (1, 0));
        let mut state = SearchState::new(&position, &crate::PatternEvaluator);
        let mut statistics = SearchStatistics::default();
        let mut line = crate::principal_variation::PvTable::new();
        let mut selective_depth = 0;
        let distant = engine
            .negamax::<true>(
                &mut state,
                4,
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
                7,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut selective_depth,
                    pv: &mut line,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap();
        assert_eq!(distant.score, -crate::score::MATE_SCORE + 9);
        assert_eq!(statistics.tt_probes, 0);
        let several = fixture(&[
            0, 108, 2, 109, 4, 110, 6, 111, 30, 168, 32, 169, 34, 170, 36, 171,
        ]);
        let result = engine.search(&several, SearchLimits::new(4));
        assert_eq!(result.score, score);
        assert_eq!(result.principal_variation, pv);
    }

    #[test]
    fn own_immediate_win_precedes_multiple_opponent_wins() {
        let position = fixture(&[108, 48, 109, 49, 110, 50, 111, 51]);
        let patterns = crate::PatternState::new(&position);
        assert_eq!(
            patterns
                .winning_moves(position.side_to_move().opponent())
                .iter()
                .count(),
            2
        );
        let (score, pv, stats, _) = q_result(&position, super::MAX_QSEARCH_PLY);
        assert_eq!(score, crate::score::MATE_SCORE - 1);
        assert_eq!(pv, [Move::from_index(107).unwrap()]);
        assert_eq!(stats.qnodes, 1);
        let result = AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1))
            .search(&position, SearchLimits::new(4));
        assert_eq!((result.best_move, result.score), (Some(pv[0]), score));
        assert_eq!(result.statistics.lmr_reductions, 0);
    }

    #[test]
    fn opponent_potential_four_does_not_remove_stand_pat() {
        let position = fixture(&[0, 110, 2, 111, 15, 112]);
        let patterns = crate::PatternState::new(&position);
        let enemy = position.side_to_move().opponent();
        assert!(patterns.winning_moves(enemy).is_empty());
        assert!(
            !patterns
                .moves_at_least(enemy, crate::pattern::ThreatProfile::Four)
                .is_empty()
        );
        let stand_pat = crate::PatternEvaluator.evaluate(&position, &patterns, &());
        let (score, pv, stats, _) = q_result(&position, 0);
        assert_eq!(score, stand_pat);
        assert!(pv.is_empty());
        assert_eq!((stats.qnodes, stats.static_evaluations), (1, 1));
    }

    #[test]
    fn lmr_researches_improvements_and_selective_siblings_block_upper_tt_evidence() {
        // A shallow horizon prefers late moves, while the full horizon rejects
        // them. Only a full-depth re-search can distinguish these evaluations.
        struct HorizonEvaluator;
        impl Evaluator for HorizonEvaluator {
            type State = ();
            type Undo = ();
            fn initialize(&self, _: &Position, _: &crate::PatternState) {}
            fn make_move(&self, _: &mut (), _: &crate::PatternDelta) {}
            fn unmake_move(&self, _: &mut (), _: &crate::PatternDelta, _: ()) {}
            fn evaluate(&self, _: &Position, _: &crate::PatternState, _: &()) -> i32 {
                100
            }
        }
        let position = fixture(&[112]);
        let mut state = SearchState::new(&position, &HorizonEvaluator);
        let engine = AlphaBetaEngine::with_config(HorizonEvaluator, EngineConfig::new(1));
        let mut statistics = SearchStatistics::default();
        let mut pv = crate::principal_variation::PvTable::new();
        let mut seldepth = 0;
        let score = engine
            .negamax::<true>(
                &mut state,
                3,
                0,
                1,
                0,
                &mut super::SearchResources {
                    interior_proof: None,
                    analysis: None,
                    budget: &mut crate::search_control::SearchBudget::default(),
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            )
            .unwrap();
        assert_eq!(score.score, -100);
        assert!(!score.validity.upper);
        assert!(statistics.lmr_reductions > 0);
        assert_eq!(statistics.lmr_researches, statistics.lmr_reductions);
        assert!(
            engine
                .table
                .probe(state.key().value())
                .is_none_or(|entry| { entry.depth != 3 || entry.bound != Bound::Upper }),
            "selectively skipped siblings cannot fabricate a nominal upper bound"
        );
        state.assert_consistent(&HorizonEvaluator);
        assert_eq!(state.position(), &position);
    }

    #[test]
    fn lmr_excludes_tactical_and_high_priority_moves() {
        let position = fixture(&[110, 0, 111, 2, 112, 15]);
        let patterns = crate::PatternState::new(&position);
        let side = position.side_to_move();
        let mut heuristics = crate::search_heuristics::SearchHeuristics::default();
        let mut tactical = 0;
        for at in patterns.empty_cells().iter() {
            if patterns.profile(at, side) != crate::pattern::ThreatProfile::Quiet
                || patterns.profile(at, side.opponent()) != crate::pattern::ThreatProfile::Quiet
            {
                assert_eq!(heuristics.lmr_reduction(6, 20, side, at, 0, &patterns), 0);
                tactical += 1;
            }
        }
        assert!(tactical > 0);
        let quiet = Move::from_index(224).unwrap();
        assert_eq!(
            heuristics.lmr_reduction(6, 20, side, quiet, 0, &patterns),
            1
        );
        assert_eq!(
            heuristics.lmr_reduction(2, 20, side, quiet, 0, &patterns),
            0
        );
        assert_eq!(heuristics.lmr_reduction(6, 7, side, quiet, 0, &patterns), 0);
        heuristics.record_cutoff(side, quiet, 16, 1, &patterns);
        assert_eq!(
            heuristics.lmr_reduction(6, 20, side, quiet, 0, &patterns),
            0
        );
        let mut killers = crate::search_heuristics::SearchHeuristics::default();
        killers.record_cutoff(side, quiet, 1, 0, &patterns);
        assert_eq!(killers.lmr_reduction(6, 20, side, quiet, 0, &patterns), 0);
    }

    #[test]
    fn selective_search_preserves_warm_cold_root_results_and_legal_pv() {
        for (position, depth) in [
            (fixture(&[112, 97, 128, 113]), 6),
            (fixture(&[107, 108, 0, 109, 2, 110, 15, 111]), 6),
            (fixture(&[112]), 4),
        ] {
            let mut engine =
                AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
            let cold = engine.search(&position, SearchLimits::new(depth));
            let warm = engine.search(&position, SearchLimits::new(depth));
            assert_eq!((warm.best_move, warm.score), (cold.best_move, cold.score));
            let mut replay = position.clone();
            for at in warm.principal_variation {
                replay.make_move(at).unwrap();
            }
        }
    }
    #[test]
    fn forced_block_allows_valid_tt_scores_but_never_an_unrelated_candidate() {
        let position = fixture(&[107, 108, 0, 109, 2, 110, 15, 111]);
        for (bound, score, alpha, beta) in [
            (
                Bound::Exact,
                crate::score::MATE_SCORE - 9,
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
            ),
            (Bound::Lower, 30, 10, 20),
            (Bound::Upper, -30, -20, -10),
        ] {
            let engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(1));
            let mut state = SearchState::new(&position, &ZeroEvaluator);
            let stored = crate::score::score_to_tt(score, 7);
            engine.table.store(TtEntry::new(
                state.key().value(),
                stored,
                Some(Move::from_index(224).unwrap()),
                3,
                bound,
                0,
            ));
            let mut statistics = SearchStatistics::default();
            let mut pv = crate::principal_variation::PvTable::new();
            let mut seldepth = 0;
            let mut resources = super::SearchResources {
                interior_proof: None,
                analysis: None,
                budget: &mut crate::search_control::SearchBudget::default(),
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: crate::search_heuristics::SearchHeuristics::default(),
            };
            let result = engine
                .negamax::<true>(&mut state, 3, alpha, beta, 4, &mut resources)
                .unwrap();
            assert_eq!(result.score, crate::score::score_from_tt(stored, 4));
            assert_eq!(resources.statistics.tt_cutoffs, 1);
            assert_eq!(resources.statistics.nodes, 1);
            // A mismatched depth must search exactly the block; the unrelated
            // legal hash move cannot escape the single-candidate restriction.
            resources.statistics.tt_cutoffs = 0;
            engine
                .negamax::<true>(
                    &mut state,
                    1,
                    -crate::score::SEARCH_INFINITY,
                    crate::score::SEARCH_INFINITY,
                    0,
                    &mut resources,
                )
                .unwrap();
            assert_eq!(resources.statistics.tt_cutoffs, 0);
            assert_eq!(resources.pv.root_line(), &[Move::CENTER]);
            state.assert_consistent(&ZeroEvaluator);
        }
    }

    #[test]
    fn unverified_lmr_fail_lows_propagate_to_ancestors_without_nominal_tt_storage() {
        let position = fixture(&[112]);
        for initial_counter in [0, 10_000] {
            let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(1));
            let mut state = SearchState::new(&position, &ZeroEvaluator);
            let mut statistics = SearchStatistics {
                lmr_reductions: initial_counter,
                ..SearchStatistics::default()
            };
            let mut pv = crate::principal_variation::PvTable::new();
            let mut seldepth = 0;
            let mut resources = super::SearchResources {
                interior_proof: None,
                analysis: None,
                budget: &mut crate::search_control::SearchBudget::default(),
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: crate::search_heuristics::SearchHeuristics::default(),
            };
            let result = engine
                .negamax::<true>(&mut state, 3, 0, 1, 0, &mut resources)
                .unwrap();
            assert_eq!(result.score, 0);
            assert!(!result.validity.upper);
            assert!(resources.statistics.lmr_reductions > initial_counter);
            assert_eq!(resources.statistics.lmr_researches, 0);
            assert!(engine.table.probe(state.key().value()).is_none());
            engine.clear_transposition_table();
            engine
                .search_root::<true>(&mut state, 4, -1, 0, &mut resources)
                .unwrap();
            assert!(engine.table.probe(state.key().value()).is_none());
            state.assert_consistent(&ZeroEvaluator);
        }
    }

    #[test]
    fn nominal_cutoff_after_selective_siblings_stores_only_valid_lower_bound() {
        // A scout equality supplies only an upper bound and must not repair
        // missing lower evidence for an exact result.
        let mut missing_lower = super::BoundValidity {
            lower: false,
            upper: true,
        };
        let scout = super::NodeResult::verified(10, 10, 11);
        missing_lower.include(scout.validity, 10, 10);
        assert!(!missing_lower.supports(Bound::Exact));
        use rustmoku_core::Stone;
        struct LateEvaluator(Move);
        impl Evaluator for LateEvaluator {
            type State = ();
            type Undo = ();
            fn initialize(&self, _: &Position, _: &crate::PatternState) {}
            fn make_move(&self, _: &mut (), _: &crate::PatternDelta) {}
            fn unmake_move(&self, _: &mut (), _: &crate::PatternDelta, _: ()) {}
            fn evaluate(&self, p: &Position, _: &crate::PatternState, _: &()) -> i32 {
                if p.move_count() == 3 && p.cell(self.0) == Some(Stone::White) {
                    10
                } else {
                    0
                }
            }
        }
        let position = fixture(&[112]);
        let patterns = crate::PatternState::new(&position);
        let mut moves = crate::move_generation::generate_candidates(&position);
        crate::move_ordering::order_moves(
            Stone::White,
            &patterns,
            &mut moves,
            None,
            &crate::search_heuristics::SearchHeuristics::default(),
            0,
            |_| None,
        );
        let late = moves.as_slice()[12];
        for initial_counter in [0, 50_000] {
            let engine = AlphaBetaEngine::with_config(LateEvaluator(late), EngineConfig::new(1));
            let mut state = SearchState::new(&position, &engine.evaluator);
            let undo = state.make_move(late, &engine.evaluator).unwrap();
            engine.table.store(TtEntry::new(
                state.key().value(),
                -10,
                None,
                2,
                Bound::Exact,
                0,
            ));
            state.unmake_move(undo, &engine.evaluator);
            let mut statistics = SearchStatistics {
                lmr_reductions: initial_counter,
                ..SearchStatistics::default()
            };
            let mut pv = crate::principal_variation::PvTable::new();
            let mut seldepth = 0;
            let result = engine
                .negamax::<true>(
                    &mut state,
                    3,
                    0,
                    1,
                    0,
                    &mut super::SearchResources {
                        interior_proof: None,
                        analysis: None,
                        budget: &mut crate::search_control::SearchBudget::default(),
                        seldepth: &mut seldepth,
                        pv: &mut pv,
                        statistics: &mut statistics,
                        heuristics: crate::search_heuristics::SearchHeuristics::default(),
                    },
                )
                .unwrap();
            assert_eq!(result.score, 10);
            assert!(statistics.lmr_reductions - initial_counter > statistics.lmr_researches);
            assert!(result.validity.lower && !result.validity.upper);
            assert!(!result.validity.supports(Bound::Exact));
            let cached = engine.table.probe(state.key().value()).unwrap();
            assert_eq!(
                (cached.depth, cached.bound, cached.score),
                (3, Bound::Lower, 10)
            );
            let negated = -result;
            assert!(negated.validity.upper && !negated.validity.lower);
            assert_eq!(state.position(), &position);
            state.assert_consistent(&engine.evaluator);
        }
    }
}
