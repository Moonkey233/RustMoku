use super::*;
use crate::zobrist::PositionKey;
use std::sync::atomic::{AtomicBool, Ordering};

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

#[test]
fn fallback_uses_practical_root_order_without_claiming_a_completed_score() {
    let position = fixture(&[112]);
    let result = AlphaBetaEngine::with_config(PatternEvaluator, config())
        .search(&position, SearchLimits::new(4).with_max_nodes(0));
    let state = SearchState::new(&position, &PatternEvaluator);
    let expected = state
        .candidate_bits()
        .iter()
        .max_by_key(|&at| resistance_key(position.side_to_move(), state.patterns(), at, None));
    assert_eq!(result.best_move, expected);
    assert_ne!(result.best_move, state.candidate_bits().iter().next());
    assert_eq!(result.origin, SearchOrigin::Fallback);
    assert_eq!(result.completed_depth, 0);
    assert_eq!(result.statistics.work_nodes, 0);
    assert_eq!(position, fixture(&[112]));
}

#[test]
fn negative_static_root_ties_remain_canonical_with_resistance_enabled() {
    struct EqualLoss;
    impl Evaluator for EqualLoss {
        type State = ();
        type Undo = ();
        fn initialize(&self, _: &Position, _: &crate::PatternState) {}
        fn make_move(&self, _: &mut (), _: &crate::PatternDelta) {}
        fn unmake_move(&self, _: &mut (), _: &crate::PatternDelta, _: ()) {}
        fn evaluate(&self, _: &Position, _: &crate::PatternState, _: &()) -> i32 {
            10
        }
    }
    let position = fixture(&[112]);
    let state = SearchState::new(&position, &EqualLoss);
    for enabled in [false, true] {
        let mut engine =
            AlphaBetaEngine::with_config(EqualLoss, config().with_root_resistance(enabled));
        for _ in 0..2 {
            let result = engine.search(&position, SearchLimits::new(1));
            assert_eq!(result.score, -10);
            assert_eq!(result.best_move, state.candidate_bits().iter().next());
            assert_eq!(result.statistics.root_resistance_ties, 0);
            assert_eq!(result.statistics.root_resistance_researches, 0);
            assert_eq!(result.origin, SearchOrigin::AlphaBeta);
            assert_eq!(result.proof, None);
            let entry = engine
                .table
                .probe(PositionKey::from_position(&position).value())
                .unwrap();
            assert_eq!(entry.score, -10);
        }
    }
}

#[test]
fn mate_loss_resistance_preserves_distance_and_changes_only_equal_root_ties() {
    let position = fixture(&[0, 110, 14, 111, 210, 112, 224, 140, 3, 141, 17, 142]);
    let run = |enabled| {
        AlphaBetaEngine::with_config(
            PatternEvaluator,
            config()
                .with_root_resistance(enabled)
                .with_selectivity(crate::SelectivityConfig::OFF),
        )
        .search(&position, SearchLimits::new(2))
    };
    let canonical = run(false);
    let practical = run(true);
    assert_eq!(practical.score, -MATE_SCORE + 4);
    assert_eq!(practical.score, canonical.score);
    assert_ne!(practical.best_move, canonical.best_move);
    let patterns = crate::PatternState::new(&position);
    assert!(
        resistance_key(
            position.side_to_move(),
            &patterns,
            practical.best_move.unwrap(),
            None
        ) > resistance_key(
            position.side_to_move(),
            &patterns,
            canonical.best_move.unwrap(),
            None
        )
    );
    let mut replay = position.clone();
    for at in &practical.principal_variation {
        replay.make_move(*at).unwrap();
    }
    assert_eq!(replay.winner(), Some(position.side_to_move().opponent()));
}

#[test]
fn resistance_never_promotes_a_scout_bound_over_a_better_primary_score() {
    struct MisleadingPolicy;
    impl Evaluator for MisleadingPolicy {
        type State = ();
        type Undo = ();
        fn initialize(&self, _: &Position, _: &crate::PatternState) {}
        fn make_move(&self, _: &mut (), _: &crate::PatternDelta) {}
        fn unmake_move(&self, _: &mut (), _: &crate::PatternDelta, _: ()) {}
        fn evaluate(&self, position: &Position, _: &crate::PatternState, _: &()) -> i32 {
            -10 - i32::from(
                position.cell(Move::from_index(96).unwrap()) == Some(rustmoku_core::Stone::White),
            )
        }
        fn policy_score(
            &self,
            _: &Position,
            _: &crate::PatternState,
            _: &(),
            at: Move,
        ) -> Option<i32> {
            Some(i32::from(at.index() == 96))
        }
    }
    let position = fixture(&[112]);
    let engine = AlphaBetaEngine::with_config(MisleadingPolicy, config());
    engine.table.store(TtEntry::new(
        PositionKey::from_position(&position).value(),
        -10,
        Some(Move::from_index(80).unwrap()),
        2,
        Bound::Exact,
        0,
    ));
    // This valid child lower bound makes the worse, policy-preferred candidate
    // appear equal to -10 at the root until full-window verification finds -11.
    engine.table.store(TtEntry::new(
        PositionKey::from_position(&fixture(&[112, 96])).value(),
        10,
        None,
        1,
        Bound::Lower,
        0,
    ));
    let mut state = SearchState::new(&position, &MisleadingPolicy);
    let mut context = engine.ab_context();
    context.root_resistance = true;
    let mut statistics = SearchStatistics::default();
    let result = context
        .search_root::<true>(
            &mut state,
            2,
            -SEARCH_INFINITY,
            SEARCH_INFINITY,
            &mut SearchResources {
                interior_proof: None,
                analysis: None,
                budget: &mut SearchBudget::default(),
                seldepth: &mut 0,
                pv: &mut PvTable::new(),
                statistics: &mut statistics,
                heuristics: SearchHeuristics::default(),
            },
        )
        .unwrap();
    assert_eq!(result.score, -10);
    assert_ne!(result.best_move, Some(Move::from_index(96).unwrap()));
    assert_eq!(statistics.root_resistance_researches, 0);
    assert_eq!(state.position(), &position);
}

fn same_iteration(result: &SearchResult, info: &SearchInfo) {
    assert_eq!(result.completed_depth, info.completed_depth);
    assert_eq!(result.seldepth, info.seldepth);
    assert_eq!(result.best_move, info.best_move);
    assert_eq!(result.score, info.score);
    assert_eq!(result.principal_variation, info.principal_variation);
    assert_eq!(result.origin, info.origin);
}

#[test]
fn root_completion_paths_report_explicit_origins() {
    let empty = Position::default();
    let analysis = AlphaBetaEngine::with_config(PatternEvaluator, config())
        .search(&empty, SearchLimits::new(0));
    assert_eq!(analysis.origin, SearchOrigin::Analysis);

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let fallback = AlphaBetaEngine::with_config(PatternEvaluator, config()).search_controlled(
        &empty,
        SearchLimits::new(1),
        cancellation,
        &mut |_| {},
    );
    assert_eq!(fallback.origin, SearchOrigin::Fallback);

    let ordinary = fixture(&[112, 97, 128, 113]);
    let alpha_beta = AlphaBetaEngine::with_config(PatternEvaluator, config())
        .search(&ordinary, SearchLimits::new(1));
    assert_eq!(alpha_beta.origin, SearchOrigin::AlphaBeta);

    let mut immediate = fixture(&[0, 15, 1, 16, 2, 17, 3, 30]);
    let tactic = AlphaBetaEngine::with_config(PatternEvaluator, config())
        .search(&immediate, SearchLimits::new(1));
    assert_eq!(tactic.origin, SearchOrigin::Immediate);
    immediate.make_move(Move::from_index(4).unwrap()).unwrap();
    let terminal = AlphaBetaEngine::with_config(PatternEvaluator, config())
        .search(&immediate, SearchLimits::new(1));
    assert_eq!(terminal.origin, SearchOrigin::Terminal);
}

#[test]
fn replacing_an_evaluator_clears_evaluator_dependent_tt_entries() {
    let position = fixture(&[112, 97, 128, 113]);
    let mut engine = AlphaBetaEngine::with_config(PatternEvaluator, config());
    engine.search(&position, SearchLimits::new(2));
    let key = PositionKey::from_position(&position).value();
    assert!(engine.table.probe(key).is_some());
    engine.replace_evaluator(PatternEvaluator);
    assert!(engine.table.probe(key).is_none());
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
fn lazy_smp_aggregates_worker_work_and_returns_a_legal_principal_pv() {
    let position = fixture(&[112, 97, 128, 113]);
    let config = config().with_threads(4);
    let result = AlphaBetaEngine::with_config(PatternEvaluator, config)
        .search(&position, SearchLimits::new(3));
    assert_eq!(result.termination, SearchTermination::Completed);
    assert_eq!(result.statistics.worker_count, 4);
    assert_eq!(
        result.statistics.nodes,
        result.statistics.principal_nodes + result.statistics.helper_nodes
    );
    assert_eq!(
        result.statistics.work_nodes,
        result.statistics.nodes + result.statistics.vcf_nodes + result.statistics.vct_nodes
    );
    assert_eq!(
        result.principal_variation.first().copied(),
        result.best_move
    );
    let mut replay = position.clone();
    for at in result.principal_variation {
        replay
            .make_move(at)
            .expect("parallel principal variation is legal");
    }
}

#[test]
fn lazy_smp_global_node_limit_is_exact_across_workers() {
    let position = fixture(&[112, 97, 128, 113]);
    let result = AlphaBetaEngine::with_config(PatternEvaluator, config().with_threads(4))
        .search(&position, SearchLimits::new(8).with_max_nodes(500));
    assert_eq!(result.termination, SearchTermination::NodeLimit);
    assert_eq!(result.statistics.work_nodes, 500);
    assert!(result.statistics.nodes <= result.statistics.work_nodes);
    assert_eq!(
        result.statistics.nodes,
        result.statistics.principal_nodes + result.statistics.helper_nodes
    );
    assert!(
        position.is_legal(
            result
                .best_move
                .expect("positive depth has a fallback move")
        )
    );
}

#[test]
fn helper_shutdown_after_principal_completion_is_not_public_cancellation() {
    let position = fixture(&[112]);
    let result = AlphaBetaEngine::with_config(PatternEvaluator, config().with_threads(4))
        .search(&position, SearchLimits::new(1));
    assert_eq!(result.termination, SearchTermination::Completed);
    assert_eq!(result.completed_depth, 1);
}

#[test]
fn parallel_cancellation_keeps_only_the_principal_workers_completed_depth() {
    let position = fixture(&[112, 97, 128, 113]);
    let armed = AtomicBool::new(false);
    let cancellation = CancellationToken::new();
    let evaluator = AuditEvaluator {
        armed: &armed,
        cancellation: cancellation.clone(),
        delay: None,
    };
    let mut infos = Vec::new();
    let result = AlphaBetaEngine::with_config(evaluator, config().with_threads(4))
        .search_controlled(&position, SearchLimits::new(8), cancellation, &mut |info| {
            infos.push(info);
            armed.store(true, Ordering::Relaxed);
        });
    assert_eq!(result.termination, SearchTermination::Cancelled);
    let last = infos.last().expect("depth one must complete before arming");
    same_iteration(&result, last);
    assert!(result.statistics.work_nodes > last.statistics.work_nodes);
    assert_eq!(
        result.statistics.nodes,
        result.statistics.principal_nodes + result.statistics.helper_nodes
    );
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
    armed: &'a AtomicBool,
    cancellation: CancellationToken,
    delay: Option<Duration>,
}

impl Evaluator for AuditEvaluator<'_> {
    type State = usize;
    type Undo = usize;
    fn initialize(&self, position: &Position, _patterns: &crate::PatternState) -> usize {
        Move::all()
            .filter(|&at| position.cell(at).is_some())
            .map(|at| at.index() + 1)
            .sum()
    }
    fn make_move(&self, state: &mut usize, delta: &crate::PatternDelta) -> usize {
        let undo = *state;
        let at = delta.played_move();
        *state += at.index() + 1;
        undo
    }
    fn unmake_move(&self, state: &mut usize, _delta: &crate::PatternDelta, undo: usize) {
        *state = undo;
    }
    fn evaluate(&self, position: &Position, patterns: &crate::PatternState, state: &usize) -> i32 {
        assert_eq!(*state, self.initialize(position, patterns));
        if self.armed.swap(false, Ordering::Relaxed) {
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
    let armed = AtomicBool::new(false);
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
            armed.store(true, Ordering::Relaxed);
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
    assert_eq!(zero.origin, SearchOrigin::Fallback);
    let armed = AtomicBool::new(false);
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
            armed.store(true, Ordering::Relaxed);
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
    let armed = AtomicBool::new(false);
    for cap in [1, 20, 256] {
        let evaluator = AuditEvaluator {
            armed: &armed,
            cancellation: CancellationToken::new(),
            delay: None,
        };
        let engine = AlphaBetaEngine::with_config(evaluator, config());
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
                interior_proof: None,
                analysis: None,
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
                interior_proof: None,
                analysis: None,
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
    for (indices, expected_origin) in [
        (
            &[108, 107, 109, 0, 110, 2, 66, 4, 81, 6][..],
            SearchOrigin::Vcf,
        ),
        (&[110, 0, 111, 14, 82, 210, 97, 224][..], SearchOrigin::Vct),
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
        let config = if expected_origin == SearchOrigin::Vcf {
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
        assert_eq!(proof.origin, expected_origin);
        assert_eq!(infos.len(), 1);
        assert!(infos[0].proof.is_some());
        same_iteration(&proof, &infos[0]);
        assert_eq!(
            proof.statistics.work_nodes,
            proof.statistics.nodes + proof.statistics.vcf_nodes + proof.statistics.vct_nodes
        );
    }
}

#[test]
fn worker_scratch_survives_interruptions_and_matches_fresh_history() {
    let position = fixture(&[112, 97, 128, 113]);
    let mut engine = AlphaBetaEngine::with_config(PatternEvaluator, config());
    let limits = SearchLimits::new(3);
    let expected = engine.search(&position, limits);
    let addresses = engine
        .scratch
        .as_ref()
        .unwrap()
        .heuristics
        .as_ref()
        .unwrap()
        .storage_addresses();
    engine.search(&position, SearchLimits::new(8).with_max_nodes(150));
    engine.clear_transposition_table();
    let actual = engine.search(&position, limits);
    assert_eq!(
        (actual.best_move, actual.score, actual.principal_variation),
        (
            expected.best_move,
            expected.score,
            expected.principal_variation
        )
    );
    assert_eq!(
        engine
            .scratch
            .as_ref()
            .unwrap()
            .heuristics
            .as_ref()
            .unwrap()
            .storage_addresses(),
        addresses
    );
    engine.reconfigure(config().with_threads(3));
    engine.search(&position, SearchLimits::new(3).with_max_nodes(500));
    let helpers: Vec<_> = engine
        .helper_scratch
        .iter()
        .map(|s| s.heuristics.as_ref().unwrap().storage_addresses())
        .collect();
    engine.search(&position, SearchLimits::new(3).with_max_nodes(500));
    assert_eq!(
        helpers,
        engine
            .helper_scratch
            .iter()
            .map(|s| s.heuristics.as_ref().unwrap().storage_addresses())
            .collect::<Vec<_>>()
    );
}
