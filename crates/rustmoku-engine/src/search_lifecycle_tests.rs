use super::*;
use rustmoku_core::Stone;
use std::cell::Cell;

fn fixture(indices: &[usize]) -> Position {
    let mut position = Position::default();
    for &index in indices {
        position
            .make_move(Move::from_index(index).unwrap())
            .unwrap();
    }
    position
}

fn config() -> EngineConfig {
    EngineConfig::new(1)
        .with_vcf_limits(0, 0)
        .with_vct_limits(0, 0)
        .with_vct_table_memory(0)
}

fn same_iteration(result: &SearchResult, info: &SearchInfo) {
    assert_eq!(result.completed_depth, info.completed_depth);
    assert_eq!(result.seldepth, info.seldepth);
    assert_eq!(result.best_move, info.best_move);
    assert_eq!(result.score, info.score);
    assert_eq!(result.principal_variation, info.principal_variation);
}

#[test]
fn node_limit_is_exact_deterministic_and_retains_last_complete_iteration() {
    let position = fixture(&[112, 97, 128, 113]);
    let run = || {
        let mut infos = Vec::new();
        let result = AlphaBetaEngine::with_config(PatternEvaluator, config()).search_controlled(
            &position,
            SearchLimits::new(8).with_max_nodes(500),
            CancellationToken::new(),
            &mut |info| infos.push(info),
        );
        assert_eq!(result.termination, SearchTermination::NodeLimit);
        assert_eq!(result.statistics.work_nodes, 500);
        assert_eq!(result.statistics.nodes, 500); // qnodes is a subset, not additive.
        assert!(result.statistics.qnodes > 0);
        same_iteration(&result, infos.last().unwrap());
        result
    };
    assert_eq!(run(), run());
    let zero = AlphaBetaEngine::with_config(PatternEvaluator, config())
        .search(&position, SearchLimits::new(8).with_max_nodes(0));
    assert_eq!((zero.completed_depth, zero.statistics.work_nodes), (0, 0));
    assert!(position.is_legal(zero.best_move.unwrap()));
    assert_eq!(zero.principal_variation, vec![zero.best_move.unwrap()]);
}

#[test]
fn uncontrolled_search_retains_v07_result_and_info_only_reports_completed_depths() {
    let position = fixture(&[112, 97, 128, 113]);
    let mut infos = Vec::new();
    let result = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1))
        .search_controlled(
            &position,
            SearchLimits::new(4),
            CancellationToken::new(),
            &mut |info| infos.push(info),
        );
    assert_eq!((result.best_move.unwrap().index(), result.score), (96, 780));
    assert_eq!(result.termination, SearchTermination::Completed);
    assert_eq!(
        infos.iter().map(|i| i.completed_depth).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    same_iteration(&result, infos.last().unwrap());
    for info in infos {
        let mut replay = position.clone();
        assert_eq!(info.principal_variation.first().copied(), info.best_move);
        for at in info.principal_variation {
            replay.make_move(at).unwrap();
        }
    }
    let ordinary = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1))
        .search(&position, SearchLimits::new(4));
    assert_eq!(result, ordinary);
}

// Stateful accumulator makes restoration observable independently of the
// production unit evaluators. Hooks deterministically interrupt *inside* an
// iteration after the observer has armed them at the previous completed depth.
struct AuditEvaluator<'a> {
    armed: &'a Cell<bool>,
    cancellation: CancellationToken,
    delay: Option<Duration>,
}

impl Evaluator for AuditEvaluator<'_> {
    type State = usize;
    type Undo = usize;
    fn initialize(&self, position: &Position) -> usize {
        Move::all()
            .filter(|&at| position.cell(at).is_some())
            .map(|at| at.index() + 1)
            .sum()
    }
    fn make_move(&self, state: &mut usize, at: Move, _stone: Stone) -> usize {
        let undo = *state;
        *state += at.index() + 1;
        undo
    }
    fn unmake_move(&self, state: &mut usize, undo: usize) {
        *state = undo;
    }
    fn evaluate(&self, position: &Position, patterns: &crate::PatternState, state: &usize) -> i32 {
        assert_eq!(*state, self.initialize(position));
        if self.armed.replace(false) {
            if let Some(delay) = self.delay {
                std::thread::sleep(delay);
            } else {
                self.cancellation.cancel();
            }
        }
        PatternEvaluator.evaluate(position, patterns, &())
    }
}

#[test]
fn cancellation_before_and_inside_search_never_publishes_partial_iterations() {
    let position = Position::default();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut infos = Vec::new();
    let before = AlphaBetaEngine::with_config(PatternEvaluator, config()).search_controlled(
        &position,
        SearchLimits::new(8),
        cancellation,
        &mut |info| infos.push(info),
    );
    assert_eq!(before.termination, SearchTermination::Cancelled);
    assert_eq!(
        (before.completed_depth, before.statistics.work_nodes),
        (0, 0)
    );
    assert_eq!(before.best_move, Some(Move::CENTER));
    assert!(infos.is_empty());
    let armed = Cell::new(false);
    let cancellation = CancellationToken::new();
    let evaluator = AuditEvaluator {
        armed: &armed,
        cancellation: cancellation.clone(),
        delay: None,
    };
    let result = AlphaBetaEngine::with_config(evaluator, config()).search_controlled(
        &position,
        SearchLimits::new(8),
        cancellation,
        &mut |info| {
            infos.push(info);
            armed.set(true);
        },
    );
    assert_eq!(result.termination, SearchTermination::Cancelled);
    assert_eq!(infos.len(), 1);
    same_iteration(&result, &infos[0]);
    assert!(result.statistics.work_nodes > infos[0].statistics.work_nodes);
    assert!(result.statistics.work_nodes - infos[0].statistics.work_nodes <= 256);
}

#[test]
fn deadline_inside_an_iteration_returns_previous_depth_and_zero_time_falls_back() {
    let position = Position::default();
    let zero = AlphaBetaEngine::with_config(PatternEvaluator, config()).search(
        &position,
        SearchLimits::new(8).with_move_time(Duration::ZERO),
    );
    assert_eq!(zero.termination, SearchTermination::TimeLimit);
    assert_eq!(zero.completed_depth, 0);
    let armed = Cell::new(false);
    // Depth one on an empty board visits two nodes. The long margin avoids
    // wall-clock races during that setup, then the evaluator crosses the limit.
    let evaluator = AuditEvaluator {
        armed: &armed,
        cancellation: CancellationToken::new(),
        delay: Some(Duration::from_millis(350)),
    };
    let mut infos = Vec::new();
    let result = AlphaBetaEngine::with_config(evaluator, config()).search_controlled(
        &position,
        SearchLimits::new(8).with_move_time(Duration::from_millis(300)),
        CancellationToken::new(),
        &mut |info| {
            infos.push(info);
            armed.set(true);
        },
    );
    assert_eq!(result.termination, SearchTermination::TimeLimit);
    assert_eq!(infos.len(), 1);
    same_iteration(&result, &infos[0]);
    assert!(result.statistics.work_nodes > infos[0].statistics.work_nodes);
}

#[test]
fn interrupted_recursion_restores_all_sidecars_and_does_not_store_root_bound() {
    let position = fixture(&[112, 97, 128, 113]);
    let armed = Cell::new(false);
    for cap in [1, 20, 256] {
        let evaluator = AuditEvaluator {
            armed: &armed,
            cancellation: CancellationToken::new(),
            delay: None,
        };
        let mut engine = AlphaBetaEngine::with_config(evaluator, config());
        let mut state = SearchState::new(&position, &engine.evaluator);
        let mut budget = SearchBudget::new(
            SearchLimits::new(6).with_max_nodes(cap),
            CancellationToken::new(),
        );
        let mut pv = PvTable::new();
        let mut statistics = SearchStatistics::default();
        let result = engine.search_root::<true>(
            &mut state,
            6,
            -SEARCH_INFINITY,
            SEARCH_INFINITY,
            &mut SearchResources {
                seldepth: &mut 0,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: SearchHeuristics::default(),
                budget: &mut budget,
            },
        );
        assert_eq!(result, Err(Stopped));
        assert_eq!(state.position(), &position);
        state.assert_consistent(&engine.evaluator);
        assert!(engine.table.probe(state.key().value()).is_none());
    }
    // Exercise the uncapped forced-block qsearch unwind as well.
    let position = fixture(&[107, 108, 0, 109, 2, 110, 15, 111]);
    let engine = AlphaBetaEngine::with_config(PatternEvaluator, config());
    let mut state = SearchState::new(&position, &PatternEvaluator);
    let mut budget = SearchBudget::new(
        SearchLimits::new(0).with_max_nodes(1),
        CancellationToken::new(),
    );
    assert_eq!(
        engine.qsearch(
            &mut state,
            -SEARCH_INFINITY,
            SEARCH_INFINITY,
            0,
            MAX_QSEARCH_PLY,
            &mut SearchResources {
                seldepth: &mut 0,
                pv: &mut PvTable::new(),
                statistics: &mut SearchStatistics::default(),
                heuristics: SearchHeuristics::default(),
                budget: &mut budget
            }
        ),
        Err(Stopped)
    );
    assert_eq!(state.position(), &position);
    state.assert_consistent(&PatternEvaluator);
}

#[test]
fn proof_work_shares_outer_limit_but_local_exhaustion_falls_through_and_proofs_emit_info() {
    for (indices, vcf) in [
        (&[108, 107, 109, 0, 110, 2, 66, 4, 81, 6][..], true),
        (&[110, 0, 111, 14, 82, 210, 97, 224][..], false),
    ] {
        let position = fixture(indices);
        let config = EngineConfig::new(1);
        let limited = AlphaBetaEngine::with_config(PatternEvaluator, config)
            .search(&position, SearchLimits::new(2).with_max_nodes(5));
        assert_eq!(limited.termination, SearchTermination::NodeLimit);
        assert_eq!(limited.statistics.work_nodes, 5);
        assert_eq!(limited.completed_depth, 0);
        assert_eq!(
            limited.statistics.vcf_budget_exhausted + limited.statistics.vct_budget_exhausted,
            0
        );
        let config = if vcf {
            config.with_vcf_limits(11, 1).with_vct_limits(0, 0)
        } else {
            config.with_vct_limits(9, 1)
        };
        let local = AlphaBetaEngine::with_config(PatternEvaluator, config)
            .search(&position, SearchLimits::new(1));
        assert_eq!(local.termination, SearchTermination::Completed);
        assert_eq!(local.completed_depth, 1);
        assert_eq!(
            local.statistics.vcf_budget_exhausted + local.statistics.vct_budget_exhausted,
            1
        );
        let mut infos = Vec::new();
        let proof = AlphaBetaEngine::with_config(PatternEvaluator, EngineConfig::new(1))
            .search_controlled(
                &position,
                SearchLimits::new(2),
                CancellationToken::new(),
                &mut |info| infos.push(info),
            );
        assert_eq!(proof.termination, SearchTermination::Completed);
        assert_eq!(infos.len(), 1);
        assert!(infos[0].tactical_proof.is_some());
        same_iteration(&proof, &infos[0]);
        assert_eq!(
            proof.statistics.work_nodes,
            proof.statistics.nodes + proof.statistics.vcf_nodes + proof.statistics.vct_nodes
        );
    }
}
