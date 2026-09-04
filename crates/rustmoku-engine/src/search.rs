use rustmoku_core::{Move, Position};

use crate::{
    ClassicalEvaluator, EngineConfig, Evaluator,
    move_generation::generate_candidates,
    move_ordering::order_moves,
    principal_variation::PvTable,
    score::{MATE_SCORE, SEARCH_INFINITY, score_from_tt, score_to_tt},
    search_state::SearchState,
    transposition_table::{Bound, TranspositionTable, TtEntry},
};

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
    pub nodes: u64,
    pub static_evaluations: u64,
    pub beta_cutoffs: u64,
    pub tt_probes: u64,
    pub tt_hits: u64,
    pub tt_cutoffs: u64,
    pub tt_stores: u64,
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

pub struct AlphaBetaEngine<E = ClassicalEvaluator> {
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

    fn begin_search_generation(&mut self) {
        if self.generation == u8::MAX {
            self.clear_transposition_table();
        }
        self.generation += 1;
    }
}

impl Default for AlphaBetaEngine<ClassicalEvaluator> {
    fn default() -> Self {
        Self::new(ClassicalEvaluator)
    }
}

impl<E: Evaluator> SearchEngine for AlphaBetaEngine<E> {
    fn search(&mut self, position: &Position, limits: SearchLimits) -> SearchResult {
        self.begin_search_generation();
        let mut state = SearchState::new(position);
        let mut statistics = SearchStatistics::default();
        let mut seldepth = 0;
        let mut pv = PvTable::new();

        if let Some(score) = terminal_score(state.position(), 0) {
            statistics.nodes = 1;
            return search_result(None, score, limits, 0, seldepth, Vec::new(), statistics);
        }
        if limits.max_depth == 0 {
            statistics.nodes = 1;
            statistics.static_evaluations += 1;
            return search_result(
                None,
                self.evaluator.evaluate(state.position()),
                limits,
                0,
                seldepth,
                Vec::new(),
                statistics,
            );
        }

        let mut completed_depth = 0;
        let mut best_move = None;
        let mut best_score = 0;
        {
            let mut resources = SearchResources {
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
            };
            for depth in 1..=limits.max_depth {
                let iteration = self.search_root(&mut state, depth, &mut resources);
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
    fn search_root(
        &mut self,
        state: &mut SearchState,
        depth: u8,
        resources: &mut SearchResources<'_>,
    ) -> RootSearchResult {
        resources.statistics.nodes += 1;
        resources.pv.clear(0);
        resources.statistics.tt_probes += 1;
        let tt_move = self.table.probe(state.key().value()).and_then(|entry| {
            resources.statistics.tt_hits += 1;
            entry
                .best_move()
                .filter(|&at| state.position().is_legal(at))
        });
        let mut moves = generate_candidates(state.position());
        order_moves(state.position(), &mut moves, tt_move);
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;
        let mut alpha = -SEARCH_INFINITY;

        for at in moves.iter() {
            let Ok(undo) = state.make_move(at) else {
                continue;
            };
            let mut score = -self.negamax(state, depth - 1, -SEARCH_INFINITY, -alpha, 1, resources);
            if score == best_score && best_move.is_some_and(|current| at < current) {
                // The first search may return a bound equal to root alpha.
                // Resolve a canonical-lower tie with an exact full-window score.
                score = -self.negamax(
                    state,
                    depth - 1,
                    -SEARCH_INFINITY,
                    SEARCH_INFINITY,
                    1,
                    resources,
                );
            }
            state.unmake_move(undo);

            if score > best_score
                || (score == best_score && best_move.is_none_or(|current| at < current))
            {
                best_score = score;
                best_move = Some(at);
                resources.pv.update(0, at);
            }
            alpha = alpha.max(score);
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
                bound: Bound::Exact,
                ply: 0,
            },
            resources.statistics,
        );
        RootSearchResult {
            best_move,
            score: best_score,
        }
    }

    fn negamax(
        &mut self,
        state: &mut SearchState,
        depth: u8,
        mut alpha: i32,
        beta: i32,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> i32 {
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

        if depth == 0 {
            resources.statistics.static_evaluations += 1;
            let score = self.evaluator.evaluate(state.position());
            self.store_tt(
                TtStore {
                    key: state.key().value(),
                    score,
                    best_move: None,
                    depth,
                    bound: Bound::Exact,
                    ply,
                },
                resources.statistics,
            );
            return score;
        }

        let mut moves = generate_candidates(state.position());
        if moves.is_empty() {
            self.store_tt(
                TtStore {
                    key: state.key().value(),
                    score: 0,
                    best_move: None,
                    depth,
                    bound: Bound::Exact,
                    ply,
                },
                resources.statistics,
            );
            return 0;
        }
        order_moves(state.position(), &mut moves, probe.best_move);
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;

        for at in moves.iter() {
            let Ok(undo) = state.make_move(at) else {
                continue;
            };
            let score = -self.negamax(state, depth - 1, -beta, -alpha, ply + 1, resources);
            state.unmake_move(undo);

            if score > best_score
                || (score == best_score && best_move.is_none_or(|current| at < current))
            {
                best_score = score;
                best_move = Some(at);
                resources.pv.update(ply, at);
            }
            alpha = alpha.max(score);
            if alpha >= beta {
                resources.statistics.beta_cutoffs += 1;
                break;
            }
        }

        let bound = classify_bound(best_score, original_alpha, beta);
        self.store_tt(
            TtStore {
                key: state.key().value(),
                score: best_score,
                best_move,
                depth,
                bound,
                ply,
            },
            resources.statistics,
        );
        best_score
    }

    fn probe_tt(
        &self,
        state: &SearchState,
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
    }
}

struct SearchResources<'a> {
    seldepth: &'a mut u8,
    pv: &'a mut PvTable,
    statistics: &'a mut SearchStatistics,
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
    if entry.depth < depth {
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
        fn evaluate(&self, _position: &Position) -> i32 {
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
    fn insufficient_depth_probe_still_supplies_legal_hash_move() {
        let mut position = Position::default();
        position
            .make_move(Move::CENTER)
            .expect("center must be legal");
        let state = SearchState::new(&position);
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
}
