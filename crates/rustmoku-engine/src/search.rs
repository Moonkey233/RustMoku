use rustmoku_core::{Move, Position};

use crate::{
    EngineConfig, Evaluator, PatternEvaluator,
    move_generation::MoveList,
    move_ordering::order_moves,
    pattern::{ThreatProfile, stone_index},
    principal_variation::PvTable,
    score::{MATE_SCORE, MATE_THRESHOLD, SEARCH_INFINITY, score_from_tt, score_to_tt},
    search_heuristics::SearchHeuristics,
    search_state::SearchState,
    transposition_table::{Bound, TranspositionTable, TranspositionTableStatistics, TtEntry},
};

const ASPIRATION_DELTA: i32 = 10_000;
const MAX_QSEARCH_PLY: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_depth: u8,
}

impl SearchLimits {
    pub const DEFAULT_DEPTH: u8 = 4;

    #[must_use]
    pub const fn new(max_depth: u8) -> Self {
        Self { max_depth }
    }
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DEPTH)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStatistics {
    /// All visited nodes, including qnodes and re-search work.
    pub nodes: u64,
    pub qnodes: u64,
    pub pvs_researches: u64,
    pub aspiration_fail_low: u64,
    pub aspiration_fail_high: u64,
    pub static_evaluations: u64,
    pub beta_cutoffs: u64,
    pub tt_probes: u64,
    pub tt_hits: u64,
    pub tt_cutoffs: u64,
    pub tt_stores: u64,
    pub tt_replacements: u64,
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
}

pub trait SearchEngine {
    fn search(&mut self, position: &Position, limits: SearchLimits) -> SearchResult;
}

pub struct AlphaBetaEngine<E = PatternEvaluator> {
    evaluator: E,
    table: TranspositionTable,
    generation: u8,
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
        }
    }

    pub fn clear_transposition_table(&mut self) {
        self.table.clear();
        self.generation = 0;
    }

    /// Samples at most 1024 buckets regardless of configured capacity.
    #[must_use]
    pub fn transposition_table_statistics(&self) -> TranspositionTableStatistics {
        self.table.statistics()
    }

    /// Replaces the table with an empty table of the requested capacity.
    pub fn resize_transposition_table(&mut self, memory_mib: usize) {
        self.table = TranspositionTable::new(memory_mib);
        self.generation = 0;
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
    fn search(&mut self, position: &Position, limits: SearchLimits) -> SearchResult {
        self.begin_search_generation();
        let mut state = SearchState::new(position, &self.evaluator);
        let mut statistics = SearchStatistics::default();
        let mut seldepth = 0;
        let mut pv = PvTable::new();

        if let Some(score) = terminal_score(state.position(), 0) {
            statistics.nodes = 1;
            return search_result(None, score, limits, 0, seldepth, Vec::new(), statistics);
        }
        if limits.max_depth == 0 {
            let score = self.qsearch(
                &mut state,
                -SEARCH_INFINITY,
                SEARCH_INFINITY,
                0,
                0,
                &mut SearchResources {
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: SearchHeuristics::default(),
                },
            );
            // A zero nominal depth remains an analysis-only call: no root move.
            return search_result(None, score, limits, 0, seldepth, Vec::new(), statistics);
        }

        let mut completed_depth = 0;
        let mut best_move = None;
        let mut best_score = 0;
        {
            let mut resources = SearchResources {
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: SearchHeuristics::default(),
            };
            for depth in 1..=limits.max_depth {
                let iteration =
                    self.search_iteration(&mut state, depth, best_score, &mut resources);
                best_move = iteration.best_move;
                best_score = iteration.score;
                completed_depth = depth;
            }
        }

        search_result(
            best_move,
            best_score,
            limits,
            completed_depth,
            seldepth,
            pv.root_line().to_vec(),
            statistics,
        )
    }
}

impl<E: Evaluator> AlphaBetaEngine<E> {
    fn search_iteration(
        &mut self,
        state: &mut SearchState<E>,
        depth: u8,
        previous_score: i32,
        resources: &mut SearchResources<'_>,
    ) -> RootSearchResult {
        let mut delta = if depth < 2 || previous_score.abs() >= MATE_THRESHOLD {
            2 * SEARCH_INFINITY
        } else {
            ASPIRATION_DELTA
        };
        loop {
            let alpha = previous_score.saturating_sub(delta).max(-SEARCH_INFINITY);
            let beta = previous_score.saturating_add(delta).min(SEARCH_INFINITY);
            let result = self.search_root::<true>(state, depth, alpha, beta, resources);
            if result.score <= alpha {
                resources.statistics.aspiration_fail_low += 1;
            } else if result.score >= beta {
                resources.statistics.aspiration_fail_high += 1;
            } else {
                return result;
            }
            // Mate transitions skip repeated widening through the static range.
            delta = if result.score.abs() >= MATE_THRESHOLD {
                2 * SEARCH_INFINITY
            } else {
                (delta * 2).min(2 * SEARCH_INFINITY)
            };
        }
    }

    // The false specialization is used only by the small Alpha-Beta oracle
    // tests. Production always uses PVS; no public algorithm switch is needed.
    fn search_root<const PVS: bool>(
        &mut self,
        state: &mut SearchState<E>,
        depth: u8,
        mut alpha: i32,
        beta: i32,
        resources: &mut SearchResources<'_>,
    ) -> RootSearchResult {
        resources.statistics.nodes += 1;
        resources.pv.clear(0);
        let original_alpha = alpha;
        resources.statistics.tt_probes += 1;
        let tt_move = self.table.probe(state.key().value()).and_then(|entry| {
            resources.statistics.tt_hits += 1;
            entry
                .best_move()
                .filter(|&at| state.position().is_legal(at))
        });
        let side = state.position().side_to_move();
        let mut moves = state.candidates();
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            tt_move,
            &resources.heuristics,
            0,
        );
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;

        for (index, at) in moves.iter().enumerate() {
            let undo = state
                .make_move(at, &self.evaluator)
                .expect("frontier moves are legal");
            let mut score;
            if PVS && index != 0 {
                score = -self.negamax::<PVS>(state, depth - 1, -alpha - 1, -alpha, 1, resources);
                if score > alpha && score < beta {
                    resources.statistics.pvs_researches += 1;
                    score = -self.negamax::<PVS>(state, depth - 1, -beta, -alpha, 1, resources);
                }
            } else {
                score = -self.negamax::<PVS>(state, depth - 1, -beta, -alpha, 1, resources);
            }
            if score == best_score
                && best_score > original_alpha
                && best_score < beta
                && best_move.is_some_and(|current| at < current)
            {
                // Equality from a scout can be only an upper bound. A smaller
                // index replaces the exact incumbent only after resolving it.
                resources.statistics.pvs_researches += 1;
                score = -self.negamax::<PVS>(
                    state,
                    depth - 1,
                    -SEARCH_INFINITY,
                    SEARCH_INFINITY,
                    1,
                    resources,
                );
            }
            state.unmake_move(undo, &self.evaluator);
            if score > best_score
                || (score == best_score && best_move.is_none_or(|current| at < current))
            {
                best_score = score;
                best_move = Some(at);
                resources.pv.update(0, at);
            }
            alpha = alpha.max(score);
            if alpha >= beta {
                resources.statistics.beta_cutoffs += 1;
                resources
                    .heuristics
                    .record_cutoff(side, at, depth, 0, state.patterns());
                break;
            }
        }
        if best_move.is_none() {
            best_score = 0;
        }
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
        RootSearchResult {
            best_move,
            score: best_score,
        }
    }

    fn negamax<const PVS: bool>(
        &mut self,
        state: &mut SearchState<E>,
        depth: u8,
        mut alpha: i32,
        beta: i32,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> i32 {
        // Qsearch owns leaf counting and never probes/stores ordinary TT scores.
        if depth == 0 {
            return self.qsearch(state, alpha, beta, ply, 0, resources);
        }
        resources.statistics.nodes += 1;
        *resources.seldepth = (*resources.seldepth).max(ply);
        resources.pv.clear(ply);
        if let Some(score) = terminal_score(state.position(), ply) {
            return score;
        }
        let original_alpha = alpha;
        let probe = self.probe_tt(state, depth, alpha, beta, ply, resources.statistics);
        if let Some(score) = probe.cutoff_score {
            return score;
        }

        let side = state.position().side_to_move();
        let mut moves = state.candidates();
        if moves.is_empty() {
            return 0;
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
        for (index, at) in moves.iter().enumerate() {
            let undo = state
                .make_move(at, &self.evaluator)
                .expect("frontier moves are legal");
            let mut score;
            if PVS && index != 0 {
                score =
                    -self.negamax::<PVS>(state, depth - 1, -alpha - 1, -alpha, ply + 1, resources);
                if score > alpha && score < beta {
                    resources.statistics.pvs_researches += 1;
                    score =
                        -self.negamax::<PVS>(state, depth - 1, -beta, -alpha, ply + 1, resources);
                }
            } else {
                score = -self.negamax::<PVS>(state, depth - 1, -beta, -alpha, ply + 1, resources);
            }
            state.unmake_move(undo, &self.evaluator);
            if score > best_score {
                best_score = score;
                best_move = Some(at);
                resources.pv.update(ply, at);
            }
            alpha = alpha.max(score);
            if alpha >= beta {
                resources.statistics.beta_cutoffs += 1;
                resources
                    .heuristics
                    .record_cutoff(side, at, depth, ply, state.patterns());
                break;
            }
        }
        self.store_tt(
            TtStore {
                key: state.key().value(),
                score: best_score,
                best_move,
                depth,
                bound: classify_bound(best_score, original_alpha, beta),
                ply,
            },
            resources.statistics,
        );
        best_score
    }

    fn qsearch(
        &self,
        state: &mut SearchState<E>,
        mut alpha: i32,
        beta: i32,
        ply: u8,
        qply: u8,
        resources: &mut SearchResources<'_>,
    ) -> i32 {
        resources.statistics.nodes += 1;
        resources.statistics.qnodes += 1;
        *resources.seldepth = (*resources.seldepth).max(ply);
        resources.pv.clear(ply);
        if let Some(score) = terminal_score(state.position(), ply) {
            return score;
        }
        if qply == MAX_QSEARCH_PLY {
            resources.statistics.static_evaluations += 1;
            return state.evaluate(&self.evaluator);
        }

        let side = state.position().side_to_move();
        let patterns = state.patterns();
        let counts = patterns.counts();
        let own_wins = counts[stone_index(side)][ThreatProfile::WinningMove as usize] != 0;
        let enemy_counts = &counts[stone_index(side.opponent())];
        let enemy_wins = enemy_counts[ThreatProfile::WinningMove as usize] != 0;
        let must_defend = enemy_counts[ThreatProfile::Four as usize..]
            .iter()
            .any(|&count| count != 0);
        let mut best_score = -SEARCH_INFINITY;
        // Never stand pat instead of winning or answering an immediate threat.
        // Also search defenses when the opponent can create a forcing four.
        if !own_wins && !must_defend {
            resources.statistics.static_evaluations += 1;
            best_score = state.evaluate(&self.evaluator);
            if best_score >= beta {
                return best_score;
            }
            alpha = alpha.max(best_score);
            if !counts[stone_index(side)][ThreatProfile::Four as usize..]
                .iter()
                .any(|&count| count != 0)
            {
                return best_score;
            }
        }
        let mut moves = MoveList::new();
        for at in state.candidates().iter() {
            let own = patterns.profile(at, side);
            let enemy = patterns.profile(at, side.opponent());
            let forcing = if own_wins {
                own == ThreatProfile::WinningMove
            } else if enemy_wins {
                enemy == ThreatProfile::WinningMove
            } else {
                own >= ThreatProfile::Four || enemy >= ThreatProfile::Four
            };
            if forcing {
                moves.push(at);
            }
        }
        order_moves(side, patterns, &mut moves, None, &resources.heuristics, ply);
        for at in moves.iter() {
            let undo = state
                .make_move(at, &self.evaluator)
                .expect("forcing frontier moves are legal");
            let score = -self.qsearch(state, -beta, -alpha, ply + 1, qply + 1, resources);
            state.unmake_move(undo, &self.evaluator);
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
        best_score
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

    fn store_tt(&mut self, store: TtStore, statistics: &mut SearchStatistics) {
        let previous_replacements = self.table.replacements();
        let entry = TtEntry::new(
            store.key,
            score_to_tt(store.score, store.ply),
            store.best_move,
            store.depth,
            store.bound,
            self.generation,
        );
        if self.table.store(entry) {
            statistics.tt_stores += 1;
        }
        statistics.tt_replacements += self.table.replacements() - previous_replacements;
    }
}

struct SearchResources<'a> {
    seldepth: &'a mut u8,
    pv: &'a mut PvTable,
    statistics: &'a mut SearchStatistics,
    heuristics: SearchHeuristics,
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
        AlphaBetaEngine, SearchEngine, SearchLimits, SearchStatistics, classify_bound,
        tt_cutoff_score,
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
    fn actual_search_recursion_restores_all_incremental_state() {
        use crate::{PatternEvaluator, principal_variation::PvTable};
        let mut position = Position::default();
        for index in [112, 97, 128, 113, 127, 98] {
            position
                .make_move(Move::from_index(index).unwrap())
                .unwrap();
        }
        let mut state = SearchState::new(&position, &PatternEvaluator);
        let mut engine = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1));
        let mut statistics = SearchStatistics::default();
        let mut pv = PvTable::new();
        let mut seldepth = 0;
        let mut resources = super::SearchResources {
            statistics: &mut statistics,
            pv: &mut pv,
            seldepth: &mut seldepth,
            heuristics: crate::search_heuristics::SearchHeuristics::default(),
        };
        for depth in 1..=3 {
            engine.search_root::<true>(
                &mut state,
                depth,
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
                &mut resources,
            );
            state.assert_consistent(&PatternEvaluator);
            assert_eq!(state.position(), &position);
        }
    }

    #[test]
    fn occupied_tt_move_is_not_used_for_ordering() {
        let mut position = Position::default();
        position.make_move(Move::CENTER).unwrap();
        let state = SearchState::new(&position, &ZeroEvaluator);
        let mut engine = AlphaBetaEngine::with_config(ZeroEvaluator, EngineConfig::new(0));
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

    fn fixed_search<const PVS: bool>(position: &Position, depth: u8) -> super::RootSearchResult {
        let mut engine =
            AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
        let mut state = SearchState::new(position, &crate::PatternEvaluator);
        let mut statistics = SearchStatistics::default();
        let mut pv = crate::principal_variation::PvTable::new();
        let mut seldepth = 0;
        engine.search_root::<PVS>(
            &mut state,
            depth,
            -crate::score::SEARCH_INFINITY,
            crate::score::SEARCH_INFINITY,
            &mut super::SearchResources {
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: crate::search_heuristics::SearchHeuristics::default(),
            },
        )
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
            let mut engine =
                AlphaBetaEngine::with_config(crate::PatternEvaluator, EngineConfig::new(1));
            let mut state = SearchState::new(&position, &crate::PatternEvaluator);
            let mut statistics = SearchStatistics::default();
            let mut pv = crate::principal_variation::PvTable::new();
            let mut seldepth = 0;
            let result = engine.search_iteration(
                &mut state,
                2,
                reference.score + offset,
                &mut super::SearchResources {
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            );
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
        let mut engine = AlphaBetaEngine::with_config(PenaltyEvaluator, EngineConfig::new(1));
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
            let result = engine.search_root::<true>(
                &mut state,
                2,
                -crate::score::SEARCH_INFINITY,
                crate::score::SEARCH_INFINITY,
                &mut super::SearchResources {
                    seldepth: &mut seldepth,
                    pv: &mut pv,
                    statistics: &mut statistics,
                    heuristics: crate::search_heuristics::SearchHeuristics::default(),
                },
            );
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
        let score = engine.qsearch(
            &mut state,
            -crate::score::SEARCH_INFINITY,
            crate::score::SEARCH_INFINITY,
            0,
            qply,
            &mut super::SearchResources {
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: crate::search_heuristics::SearchHeuristics::default(),
            },
        );
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
        assert!(stats.qnodes > 3);
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
}
