use rustmoku_core::{Move, Position};
use std::{sync::Arc, time::Duration};

use crate::{
    CancellationToken, EngineConfig, Evaluator, PatternEvaluator, Proof, ProofDistance,
    ProofSource, SearchTermination, VerifiedProofBook,
    move_generation::MoveList,
    move_ordering::order_moves,
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

const ASPIRATION_DELTA: i32 = 10_000;
const MAX_QSEARCH_PLY: u8 = 6;

#[cfg(test)]
#[path = "search_lifecycle_tests.rs"]
mod lifecycle_tests;

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
    pub max_qply: u8,
    pub pvs_researches: u64,
    pub lmr_reductions: u64,
    pub lmr_researches: u64,
    pub lmp_pruned_moves: u64,
    pub futility_pruned_moves: u64,
    pub rfp_attempts: u64,
    pub rfp_cutoffs: u64,
    pub razor_attempts: u64,
    pub razor_cutoffs: u64,
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
    pub termination: SearchTermination,
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
        }
    }
}

/// Called only at completed root events; dispatch is outside recursive search.
pub trait SearchObserver {
    fn on_info(&mut self, info: SearchInfo);
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
    /// completes, a nonterminal positive-depth search uses the lowest candidate
    /// (center on an empty board), static score and one-move fallback PV.
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
        }
    }

    pub fn clear_transposition_table(&mut self) {
        self.table.clear();
        self.generation = 0;
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
        let mut result = self.search_with_budget(
            position,
            &mut state,
            limits,
            &mut budget,
            &mut statistics,
            observer,
        );
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
    fn search_with_budget(
        &mut self,
        root_position: &Position,
        state: &mut SearchState<E>,
        limits: SearchLimits,
        budget: &mut SearchBudget,
        statistics: &mut SearchStatistics,
        observer: &mut dyn SearchObserver,
    ) -> SearchResult {
        let mut seldepth = 0;
        let mut pv = PvTable::new();
        if let Some(score) = terminal_score(state.position(), 0) {
            // Exact facts remain usable even if admission is already stopped.
            // Never exceed the cap merely to account for a known root fact.
            statistics.nodes = u64::from(budget.charge().is_ok());
            return search_result(None, score, limits, 0, 0, Vec::new(), *statistics);
        }
        let side = state.position().side_to_move();
        if limits.max_depth != 0
            && let Some((at, score)) =
                immediate_tactic(state.patterns(), side).resolve(0, &mut pv, &mut seldepth)
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
            observer.on_info(SearchInfo::from(&result));
            return result;
        }
        // Static fallback is explicitly not a completed nominal search score.
        let fallback = (limits.max_depth != 0)
            .then(|| state.candidate_bits().iter().next())
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
            let outcome = self.qsearch(
                state,
                -SEARCH_INFINITY,
                SEARCH_INFINITY,
                0,
                0,
                &mut SearchResources {
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics,
                    heuristics: SearchHeuristics::default(),
                    budget,
                },
            );
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
                completed.statistics.work_nodes = budget.work_nodes();
                observer.on_info(SearchInfo::from(&completed));
                return completed;
            }
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
                completed.statistics.work_nodes = budget.work_nodes();
                observer.on_info(SearchInfo::from(&completed));
                return completed;
            }
        }
        let mut resources = SearchResources {
            seldepth: &mut seldepth,
            pv: &mut pv,
            statistics,
            heuristics: SearchHeuristics::default(),
            budget,
        };
        self.search_ordinary(
            root_position,
            state,
            limits,
            completed,
            &mut resources,
            observer,
        )
    }

    fn search_ordinary(
        &self,
        root_position: &Position,
        state: &mut SearchState<E>,
        limits: SearchLimits,
        completed: SearchResult,
        resources: &mut SearchResources<'_>,
        observer: &mut dyn SearchObserver,
    ) -> SearchResult {
        let threads = self.config.threads();
        let principal = AbContext::new(&self.evaluator, &self.table, self.generation, 0);
        if threads == 1 {
            let mut completed =
                run_principal_iterations(&principal, state, limits, resources, completed, observer);
            resources.statistics.principal_nodes = resources.statistics.nodes;
            resources.statistics.helper_nodes = 0;
            completed.statistics = *resources.statistics;
            return completed;
        }

        let evaluator = &self.evaluator;
        let table = &self.table;
        let generation = self.generation;
        let (completed, helper_results) = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(threads.saturating_sub(1));
            for worker_id in 1..threads {
                let helper_budget = resources.budget.worker();
                let helper_position = root_position;
                let helper_evaluator = evaluator;
                let helper_table = table;
                handles.push(scope.spawn(move || {
                    let mut helper_budget = helper_budget;
                    let mut helper_state = SearchState::new(helper_position, helper_evaluator);
                    let mut helper_statistics = SearchStatistics::default();
                    let mut helper_pv = PvTable::new();
                    let mut helper_seldepth = 0;
                    let context =
                        AbContext::new(helper_evaluator, helper_table, generation, worker_id);
                    run_helper_iterations(
                        &context,
                        &mut helper_state,
                        limits,
                        &mut helper_budget,
                        &mut helper_statistics,
                        &mut helper_pv,
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
    fn add_worker(&mut self, other: Self) {
        self.nodes += other.nodes;
        self.qnodes += other.qnodes;
        self.qsearch_recursive_nodes += other.qsearch_recursive_nodes;
        self.qsearch_forcing_edges += other.qsearch_forcing_edges;
        self.qsearch_forced_blocks += other.qsearch_forced_blocks;
        self.qsearch_stand_pat_cutoffs += other.qsearch_stand_pat_cutoffs;
        self.qsearch_cap_hits += other.qsearch_cap_hits;
        self.max_qply = self.max_qply.max(other.max_qply);
        self.pvs_researches += other.pvs_researches;
        self.lmr_reductions += other.lmr_reductions;
        self.lmr_researches += other.lmr_researches;
        self.lmp_pruned_moves += other.lmp_pruned_moves;
        self.futility_pruned_moves += other.futility_pruned_moves;
        self.rfp_attempts += other.rfp_attempts;
        self.rfp_cutoffs += other.rfp_cutoffs;
        self.razor_attempts += other.razor_attempts;
        self.razor_cutoffs += other.razor_cutoffs;
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
        self.vct_cache_hits += other.vct_cache_hits;
        self.vct_proven += other.vct_proven;
        self.vct_budget_exhausted += other.vct_budget_exhausted;
    }
}

struct HelperResult {
    statistics: SearchStatistics,
    work_nodes: u64,
}

fn run_principal_iterations<E: Evaluator>(
    context: &AbContext<'_, E>,
    state: &mut SearchState<E>,
    limits: SearchLimits,
    resources: &mut SearchResources<'_>,
    mut completed: SearchResult,
    observer: &mut dyn SearchObserver,
) -> SearchResult {
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
        observer.on_info(SearchInfo::from(&completed));
    }
    completed
}

fn run_helper_iterations<E: Evaluator>(
    context: &AbContext<'_, E>,
    state: &mut SearchState<E>,
    limits: SearchLimits,
    budget: &mut SearchBudget,
    statistics: &mut SearchStatistics,
    pv: &mut PvTable,
    seldepth: &mut u8,
) {
    let mut previous_score = state.evaluate(context.evaluator);
    let mut resources = SearchResources {
        seldepth,
        pv,
        statistics,
        heuristics: SearchHeuristics::default(),
        budget,
    };
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
}

/// Borrowed immutable engine components plus worker-specific deterministic
/// root diversity. Every mutable search structure remains in the call's
/// `SearchResources` and `SearchState`.
struct AbContext<'a, E: Evaluator> {
    evaluator: &'a E,
    table: &'a TranspositionTable,
    generation: u8,
    root_rotation: usize,
}

impl<'a, E: Evaluator> AbContext<'a, E> {
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
        }
    }

    fn search_iteration(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        previous_score: i32,
        resources: &mut SearchResources<'_>,
    ) -> Result<RootSearchResult, Stopped> {
        let mut delta = if depth < 2 || previous_score.abs() >= MATE_THRESHOLD {
            2 * SEARCH_INFINITY
        } else {
            ASPIRATION_DELTA
        };
        loop {
            let alpha = previous_score.saturating_sub(delta).max(-SEARCH_INFINITY);
            let beta = previous_score.saturating_add(delta).min(SEARCH_INFINITY);
            let result = self.search_root::<true>(state, depth, alpha, beta, resources)?;
            if result.score <= alpha {
                resources.statistics.aspiration_fail_low += 1;
            } else if result.score >= beta {
                resources.statistics.aspiration_fail_high += 1;
            } else {
                return Ok(result);
            }
            // Mate transitions skip repeated widening through the static range.
            delta = if result.score.abs() >= MATE_THRESHOLD {
                2 * SEARCH_INFINITY
            } else {
                (delta * 2).min(2 * SEARCH_INFINITY)
            };
        }
    }

    // The false specialization is the small full-width, non-selective oracle
    // used by tests. Production always uses PVS; there is no public policy switch.
    fn search_root<const PVS: bool>(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        mut alpha: i32,
        beta: i32,
        resources: &mut SearchResources<'_>,
    ) -> Result<RootSearchResult, Stopped> {
        resources.budget.charge()?;
        resources.statistics.nodes += 1;
        resources.heuristics.begin_root();
        resources.pv.clear(0);
        if let Some(score) = terminal_score(state.position(), 0) {
            return Ok(RootSearchResult {
                best_move: None,
                score,
            });
        }
        let tactic = immediate_tactic(state.patterns(), state.position().side_to_move());
        if let Some((at, score)) = tactic.resolve(0, resources.pv, resources.seldepth) {
            return Ok(RootSearchResult {
                best_move: Some(at),
                score,
            });
        }
        let forced_block = tactic.forced_block();
        let mut validity = BoundValidity {
            lower: false,
            upper: true,
        };

        let original_alpha = alpha;
        resources.statistics.tt_probes += 1;
        let tt_move = self.table.probe(state.key().value()).and_then(|entry| {
            resources.statistics.tt_hits += 1;
            entry
                .best_move()
                .filter(|&at| state.position().is_legal(at))
        });
        let side = state.position().side_to_move();
        let mut moves = if let Some(at) = forced_block {
            let mut moves = MoveList::new();
            moves.push(at);
            moves
        } else {
            state.candidates()
        };
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            tt_move,
            &resources.heuristics,
            0,
        );
        if self.root_rotation != 0 && !moves.is_empty() {
            let rotation = self.root_rotation % moves.as_slice().len();
            moves.as_mut_slice().rotate_left(rotation);
        }
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;
        let mut searched_quiets = MoveList::new();

        for (index, at) in moves.iter().enumerate() {
            let quiet = SearchHeuristics::is_quiet(state.patterns(), side, at);
            resources.heuristics.set_child(1, at, index != 0, 0);
            let undo = state
                .make_move(at, self.evaluator)
                .expect("frontier moves are legal");
            let child = (|| {
                let mut result;
                if PVS && index != 0 {
                    result =
                        -self.negamax::<PVS>(state, depth - 1, -alpha - 1, -alpha, 1, resources)?;
                    if result.score > alpha && result.score < beta {
                        resources.statistics.pvs_researches += 1;
                        result =
                            -self.negamax::<PVS>(state, depth - 1, -beta, -alpha, 1, resources)?;
                    }
                } else {
                    result = -self.negamax::<PVS>(state, depth - 1, -beta, -alpha, 1, resources)?;
                }
                if result.score == best_score
                    && best_score > original_alpha
                    && best_score < beta
                    && best_move.is_some_and(|current| at < current)
                {
                    // Equality from a scout can be only an upper bound. A smaller
                    // index replaces the exact incumbent only after resolving it.
                    resources.statistics.pvs_researches += 1;
                    result = -self.negamax::<PVS>(
                        state,
                        depth - 1,
                        -SEARCH_INFINITY,
                        SEARCH_INFINITY,
                        1,
                        resources,
                    )?;
                }
                Ok::<_, Stopped>(result)
            })();
            state.unmake_move(undo, self.evaluator);
            let result = child?;
            validity.include(result.validity, result.score, best_score);
            let score = result.score;
            if quiet {
                searched_quiets.push(at);
            }
            if score > best_score
                || (score == best_score && best_move.is_none_or(|current| at < current))
            {
                best_score = score;
                best_move = Some(at);
                resources.pv.update(0, at);
            }
            alpha = alpha.max(score);
            if alpha >= beta {
                // Unsearched siblings prevent an upper bound at this node.
                validity.upper = false;
                resources.statistics.beta_cutoffs += 1;
                resources.heuristics.record_cutoff_with_context(
                    result.validity.lower,
                    side,
                    at,
                    depth,
                    0,
                    None,
                    None,
                    searched_quiets.as_slice(),
                    state.patterns(),
                );
                break;
            }
        }
        if best_move.is_none() {
            best_score = 0;
        }
        if validity.supports(classify_bound(best_score, original_alpha, beta)) {
            self.store_tt(
                TtStore {
                    key: state.key().value(),
                    score: best_score,
                    best_move,
                    depth,
                    bound: classify_bound(best_score, original_alpha, beta),
                    ply: 0,
                },
                resources.statistics,
            );
        }
        Ok(RootSearchResult {
            best_move,
            score: best_score,
        })
    }

    fn negamax<const PVS: bool>(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        mut alpha: i32,
        mut beta: i32,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<NodeResult, Stopped> {
        // Qsearch owns leaf counting and never probes/stores ordinary TT scores.
        if depth == 0 {
            return Ok(NodeResult::verified(
                self.qsearch(state, alpha, beta, ply, 0, resources)?,
                alpha,
                beta,
            ));
        }
        resources.budget.charge()?;
        resources.statistics.nodes += 1;
        *resources.seldepth = (*resources.seldepth).max(ply);
        resources.pv.clear(ply);
        resources.heuristics.begin_node(ply);
        if let Some(score) = terminal_score(state.position(), ply) {
            return Ok(NodeResult::complete(score));
        }
        let tactic = immediate_tactic(state.patterns(), state.position().side_to_move());
        if let Some((_, score)) = tactic.resolve(ply, resources.pv, resources.seldepth) {
            return Ok(NodeResult::complete(score));
        }
        let forced_block = tactic.forced_block();
        let input_alpha = alpha;
        let input_beta = beta;
        match mate_distance_window(alpha, beta, ply) {
            MateDistanceWindow::Search {
                alpha: bounded_alpha,
                beta: bounded_beta,
            } => {
                alpha = bounded_alpha;
                beta = bounded_beta;
            }
            MateDistanceWindow::Cutoff(score) => {
                return Ok(NodeResult::verified(score, input_alpha, input_beta));
            }
        }
        let mut validity = BoundValidity {
            lower: false,
            upper: true,
        };
        let original_alpha = alpha;
        let scout_node = beta == alpha + 1;
        let probe = self.probe_tt(state, depth, alpha, beta, ply, resources.statistics);
        if let Some(score) = probe.cutoff_score {
            return Ok(NodeResult::verified(score, alpha, beta));
        }
        let side = state.position().side_to_move();
        let candidate_bits = state.candidate_bits();
        let strong_threats = !candidate_bits
            .intersection(
                state
                    .patterns()
                    .moves_at_least(side, ThreatProfile::OpenThree),
            )
            .is_empty()
            || !candidate_bits
                .intersection(
                    state
                        .patterns()
                        .moves_at_least(side.opponent(), ThreatProfile::OpenThree),
                )
                .is_empty();
        let selective_node = PVS
            && scout_node
            && forced_block.is_none()
            && !strong_threats
            && alpha.abs() < MATE_THRESHOLD
            && beta.abs() < MATE_THRESHOLD;
        let static_eval = if selective_node && depth <= 3 {
            resources.statistics.static_evaluations += 1;
            let score = state.evaluate(self.evaluator);
            resources.heuristics.set_static_eval(ply, score);
            Some(score)
        } else {
            resources.heuristics.static_eval(ply)
        };
        if selective_node && depth <= 3 {
            resources.statistics.rfp_attempts += 1;
            if static_eval
                .is_some_and(|score| score - search_params::reverse_futility_margin(depth) >= beta)
            {
                resources.statistics.rfp_cutoffs += 1;
                return Ok(NodeResult::unverified(
                    static_eval.expect("computed static eval"),
                ));
            }
        }
        if selective_node
            && depth <= 2
            && static_eval.is_some_and(|score| score + search_params::razor_margin(depth) < alpha)
        {
            resources.statistics.razor_attempts += 1;
            let score = self.qsearch(state, alpha, beta, ply, 0, resources)?;
            if score <= alpha {
                resources.statistics.razor_cutoffs += 1;
                return Ok(NodeResult::unverified(score));
            }
        }
        let iir = PVS
            && scout_node
            && forced_block.is_none()
            && !strong_threats
            && probe.best_move.is_none()
            && depth >= search_params::IIR_MIN_DEPTH;
        let searched_depth = depth - u8::from(iir);
        resources.statistics.iir_reductions += u64::from(iir);
        let mut moves = if let Some(at) = forced_block {
            let mut moves = MoveList::new();
            moves.push(at);
            moves
        } else {
            state.candidates()
        };
        if moves.is_empty() {
            return Ok(NodeResult::complete(0));
        }
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            probe.best_move,
            &resources.heuristics,
            ply,
        );
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;
        let (previous, two_back) = resources.heuristics.previous_moves(ply);
        let mut searched_quiets = MoveList::new();
        for (index, at) in moves.iter().enumerate() {
            let quiet = SearchHeuristics::is_quiet(state.patterns(), side, at);
            let strong_context = resources
                .heuristics
                .is_strong_context(side, at, ply, previous, two_back);
            let late_quiet = selective_node
                && index != 0
                && quiet
                && probe.best_move != Some(at)
                && !strong_context;
            if late_quiet
                && searched_depth <= 3
                && index >= search_params::lmp_threshold(searched_depth)
            {
                resources.statistics.lmp_pruned_moves += 1;
                validity.upper = false;
                continue;
            }
            if late_quiet
                && searched_depth <= 2
                && static_eval.is_some_and(|score| {
                    score + search_params::futility_margin(searched_depth) <= alpha
                })
            {
                resources.statistics.futility_pruned_moves += 1;
                validity.upper = false;
                continue;
            }
            let extension = u8::from(threat_extension(
                state.patterns().profile(at, side),
                resources.heuristics.extensions(ply),
            ));
            resources.statistics.threat_extensions += u64::from(extension);
            let child_depth = searched_depth - 1 + extension;
            let reduction = if PVS
                && scout_node
                && forced_block.is_none()
                && probe.best_move != Some(at)
                && alpha.abs() < MATE_THRESHOLD
                && extension == 0
            {
                resources.heuristics.adaptive_lmr_reduction(
                    searched_depth,
                    index,
                    side,
                    at,
                    ply,
                    previous,
                    two_back,
                    state.patterns(),
                )
            } else {
                0
            };

            resources.heuristics.set_child(
                ply + 1,
                at,
                scout_node && index != 0,
                resources.heuristics.extensions(ply) + extension,
            );
            let undo = state
                .make_move(at, self.evaluator)
                .expect("frontier moves are legal");
            let child = (|| {
                if reduction != 0 {
                    resources.statistics.lmr_reductions += 1;
                    let reduced = -self.negamax::<PVS>(
                        state,
                        child_depth - reduction,
                        -alpha - 1,
                        -alpha,
                        ply + 1,
                        resources,
                    )?;
                    if reduced.score <= alpha {
                        return Ok((reduced, true));
                    }
                    resources.statistics.lmr_researches += 1;
                    // Improvement must survive the ordinary full-depth PVS path.
                }
                let mut result;
                if PVS && index != 0 {
                    result = -self.negamax::<PVS>(
                        state,
                        child_depth,
                        -alpha - 1,
                        -alpha,
                        ply + 1,
                        resources,
                    )?;
                    if result.score > alpha && result.score < beta {
                        resources.statistics.pvs_researches += 1;
                        result = -self.negamax::<PVS>(
                            state,
                            child_depth,
                            -beta,
                            -alpha,
                            ply + 1,
                            resources,
                        )?;
                    }
                } else {
                    result = -self.negamax::<PVS>(
                        state,
                        child_depth,
                        -beta,
                        -alpha,
                        ply + 1,
                        resources,
                    )?;
                }
                Ok::<_, Stopped>((result, false))
            })();
            state.unmake_move(undo, self.evaluator);
            let (result, reduced_fail_low) = child?;
            if reduced_fail_low {
                // Unverified reduced values cannot improve alpha/PV or support
                // nominal-depth upper bounds, including after interruption.
                validity.upper = false;
                if result.score > best_score {
                    validity.lower = false;
                }
                best_score = best_score.max(result.score);
                if quiet {
                    searched_quiets.push(at);
                }
                continue;
            }
            validity.include(result.validity, result.score, best_score);
            let score = result.score;
            if quiet {
                searched_quiets.push(at);
            }
            if score > best_score {
                best_score = score;
                best_move = Some(at);
                resources.pv.update(ply, at);
            }
            alpha = alpha.max(score);
            if alpha >= beta {
                // Unsearched siblings prevent an upper bound at this node.
                validity.upper = false;
                resources.statistics.beta_cutoffs += 1;
                resources.heuristics.record_cutoff_with_context(
                    result.validity.lower,
                    side,
                    at,
                    searched_depth,
                    ply,
                    previous,
                    two_back,
                    searched_quiets.as_slice(),
                    state.patterns(),
                );
                break;
            }
        }
        // Each bound uses only its relevant evidence. Earlier selective fail-lows
        // cannot invalidate a later verified nominal-depth cutoff child.
        if validity.supports(classify_bound(best_score, original_alpha, beta)) {
            self.store_tt(
                TtStore {
                    key: state.key().value(),
                    score: best_score,
                    best_move,
                    depth: searched_depth,
                    bound: classify_bound(best_score, original_alpha, beta),
                    ply,
                },
                resources.statistics,
            );
        }
        Ok(NodeResult {
            score: best_score,
            validity: if iir {
                BoundValidity::UNVERIFIED
            } else {
                validity
            },
        })
    }

    fn qsearch(
        &self,
        state: &mut SearchState<E>,
        mut alpha: i32,
        beta: i32,
        ply: u8,
        qply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<i32, Stopped> {
        resources.budget.charge()?;
        resources.statistics.nodes += 1;
        resources.statistics.qnodes += 1;
        resources.statistics.qsearch_recursive_nodes += u64::from(qply > 0);
        resources.statistics.max_qply = resources.statistics.max_qply.max(qply);
        *resources.seldepth = (*resources.seldepth).max(ply);
        resources.pv.clear(ply);
        resources.heuristics.begin_node(ply);
        if let Some(score) = terminal_score(state.position(), ply) {
            return Ok(score);
        }
        let side = state.position().side_to_move();
        let tactic = immediate_tactic(state.patterns(), side);
        if let Some((_, score)) = tactic.resolve(ply, resources.pv, resources.seldepth) {
            return Ok(score);
        }
        if let Some(at) = tactic.forced_block() {
            resources.statistics.qsearch_forced_blocks += 1;
            // An immediate obligation survives the expansion cap. A chain of
            // forced replies still terminates because every ply fills a cell.
            resources.heuristics.set_child(
                ply + 1,
                at,
                false,
                resources.heuristics.extensions(ply),
            );
            let undo = state
                .make_move(at, self.evaluator)
                .expect("winning point is legal");
            let child = self.qsearch(
                state,
                -beta,
                -alpha,
                ply + 1,
                (qply + 1).min(MAX_QSEARCH_PLY),
                resources,
            );
            state.unmake_move(undo, self.evaluator);
            let score = -child?;
            resources.pv.update(ply, at);
            return Ok(score);
        }
        resources.statistics.static_evaluations += 1;
        let mut best_score = state.evaluate(self.evaluator);
        if qply >= MAX_QSEARCH_PLY {
            resources.statistics.qsearch_cap_hits += 1;
            return Ok(best_score);
        }
        if best_score >= beta {
            resources.statistics.qsearch_stand_pat_cutoffs += 1;
            return Ok(best_score);
        }
        alpha = alpha.max(best_score);
        let patterns = state.patterns();
        // Only our existing forcing continuations. Potential enemy Four+
        // placements are not check, and never remove the stand-pat option.
        let noisy = state
            .candidate_bits()
            .intersection(forcing_moves(patterns, side));
        let mut moves = MoveList::new();
        for at in noisy.iter() {
            moves.push(at);
        }
        order_moves(side, patterns, &mut moves, None, &resources.heuristics, ply);
        for at in moves.iter() {
            resources.statistics.qsearch_forcing_edges += 1;
            resources.heuristics.set_child(
                ply + 1,
                at,
                false,
                resources.heuristics.extensions(ply),
            );
            let undo = state
                .make_move(at, self.evaluator)
                .expect("forcing frontier moves are legal");
            let child = self.qsearch(state, -beta, -alpha, ply + 1, qply + 1, resources);
            state.unmake_move(undo, self.evaluator);
            let score = -child?;
            if score > best_score {
                best_score = score;
                resources.pv.update(ply, at);
            }
            alpha = alpha.max(score);
            if alpha >= beta {
                resources.statistics.beta_cutoffs += 1;
                break;
            }
        }
        Ok(best_score)
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
        statistics.tt_probes += 1;
        let Some(entry) = self.table.probe(state.key().value()) else {
            return TtProbe::default();
        };
        statistics.tt_hits += 1;

        let best_move = entry
            .best_move()
            .filter(|&at| state.position().is_legal(at));
        let cutoff_score = tt_cutoff_score(entry, depth, alpha, beta, ply);
        if cutoff_score.is_some() {
            statistics.tt_cutoffs += 1;
        }
        TtProbe {
            best_move,
            cutoff_score,
        }
    }

    fn store_tt(&self, store: TtStore, statistics: &mut SearchStatistics) {
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

/// Which directions of the returned nominal-depth score are verified. Negamax
/// swaps these directions; a cutoff needs only one child's lower bound, whereas
/// an upper bound needs every relevant child's upper bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoundValidity {
    lower: bool,
    upper: bool,
}

impl BoundValidity {
    const UNVERIFIED: Self = Self {
        lower: false,
        upper: false,
    };
    const VERIFIED: Self = Self {
        lower: true,
        upper: true,
    };

    fn include(&mut self, child: Self, score: i32, best_score: i32) {
        self.upper &= child.upper;
        if score > best_score {
            self.lower = child.lower;
        } else if score == best_score {
            self.lower |= child.lower;
        }
    }

    fn supports(self, bound: Bound) -> bool {
        match bound {
            Bound::Lower => self.lower,
            Bound::Upper => self.upper,
            Bound::Exact => self.lower && self.upper,
            Bound::Empty => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct NodeResult {
    score: i32,
    validity: BoundValidity,
}

impl NodeResult {
    fn unverified(score: i32) -> Self {
        Self {
            score,
            validity: BoundValidity::UNVERIFIED,
        }
    }

    fn verified(score: i32, alpha: i32, beta: i32) -> Self {
        let bound = classify_bound(score, alpha, beta);
        Self {
            score,
            validity: BoundValidity {
                lower: bound != Bound::Upper,
                upper: bound != Bound::Lower,
            },
        }
    }

    fn complete(score: i32) -> Self {
        Self {
            score,
            validity: BoundValidity::VERIFIED,
        }
    }
}

impl std::ops::Neg for NodeResult {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            score: -self.score,
            validity: BoundValidity {
                lower: self.validity.upper,
                upper: self.validity.lower,
            },
        }
    }
}

struct SearchResources<'a> {
    seldepth: &'a mut u8,
    pv: &'a mut PvTable,
    statistics: &'a mut SearchStatistics,
    heuristics: SearchHeuristics,
    budget: &'a mut SearchBudget,
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
        fn initialize(&self, _position: &Position) {}
        fn make_move(&self, _state: &mut (), _at: Move, _stone: rustmoku_core::Stone) {}
        fn unmake_move(&self, _state: &mut (), _undo: ()) {}
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
        fn initialize(&self, _: &Position) {}
        fn make_move(&self, _: &mut (), _: Move, _: rustmoku_core::Stone) {}
        fn unmake_move(&self, _: &mut (), _: ()) {}
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
            fn initialize(&self, _: &Position) {}
            fn make_move(&self, _: &mut (), _: Move, _: rustmoku_core::Stone) {}
            fn unmake_move(&self, _: &mut (), _: ()) {}
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
            fn initialize(&self, _: &Position) {}
            fn make_move(&self, _: &mut (), _: Move, _: rustmoku_core::Stone) {}
            fn unmake_move(&self, _: &mut (), _: ()) {}
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
            fn initialize(&self, _: &Position) {}
            fn make_move(&self, _: &mut (), _: Move, _: Stone) {}
            fn unmake_move(&self, _: &mut (), _: ()) {}
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
