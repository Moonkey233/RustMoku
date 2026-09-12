use super::*;
use crate::{
    PatternDelta, PatternState, ProbCutBucket, ProbCutCalibration, ScoreContract, SearchProfile,
    SelectivityConfig,
};

#[derive(Clone, Copy)]
struct Ranked;
impl Evaluator for Ranked {
    type State = ();
    type Undo = ();
    fn model_fingerprint(&self) -> Option<[u8; 32]> {
        Some([7; 32])
    }
    fn initialize(&self, _: &Position, _: &PatternState) {}
    fn make_move(&self, _: &mut (), _: &PatternDelta) {}
    fn unmake_move(&self, _: &mut (), _: &PatternDelta, _: ()) {}
    fn evaluate(&self, _: &Position, _: &PatternState, _: &()) -> i32 {
        0
    }
    fn policy_score(&self, position: &Position, _: &PatternState, _: &(), at: Move) -> Option<i32> {
        position.is_legal(at).then_some(at.index() as i32)
    }
}

fn position(indices: &[usize]) -> Position {
    let mut p = Position::default();
    for &index in indices {
        p.make_move(Move::from_index(index).unwrap()).unwrap();
    }
    p
}
fn base() -> EngineConfig {
    EngineConfig::new(1)
        .with_vcf_limits(0, 0)
        .with_vct_limits(0, 0)
}

#[test]
fn teacher_nominal_descendants_are_all_legal_not_radius_two() {
    struct FarReply;
    impl Evaluator for FarReply {
        type State = ();
        type Undo = ();
        fn initialize(&self, _: &Position, _: &PatternState) {}
        fn make_move(&self, _: &mut (), _: &PatternDelta) {}
        fn unmake_move(&self, _: &mut (), _: &PatternDelta, _: ()) {}
        fn evaluate(&self, p: &Position, _: &PatternState, _: &()) -> i32 {
            -100 * i32::from(
                p.cell(Move::from_index(0).unwrap()) == Some(rustmoku_core::Stone::Black),
            )
        }
    }
    let p = position(&[112, 113]);
    let engine = AlphaBetaEngine::with_config(FarReply, base());
    for (domain, expected) in [(SearchDomain::Analysis, 0), (SearchDomain::Teacher, 100)] {
        let mut context = engine.ab_context();
        context.domain = domain;
        context.selectivity = SelectivityConfig::OFF;
        let mut state = SearchState::new(&p, &FarReply);
        let mut stats = SearchStatistics::default();
        let result = context
            .negamax::<false>(
                &mut state,
                1,
                -SEARCH_INFINITY,
                SEARCH_INFINITY,
                0,
                &mut SearchResources {
                    budget: &mut SearchBudget::default(),
                    statistics: &mut stats,
                    seldepth: &mut 0,
                    pv: &mut PvTable::new(),
                    heuristics: SearchHeuristics::default(),
                    interior_proof: None,
                    analysis: None,
                },
            )
            .unwrap();
        assert_eq!(result.score, expected);
        assert!(result.validity.supports(Bound::Exact));
        assert_eq!(stats.tt_probes + stats.tt_stores, 0);
        state.assert_consistent(&FarReply);
    }
}

#[test]
fn bounded_three_continuations_are_unverified_and_restore_on_stop() {
    struct Flat;
    impl Evaluator for Flat {
        type State = ();
        type Undo = ();
        fn initialize(&self, _: &Position, _: &PatternState) {}
        fn make_move(&self, _: &mut (), _: &PatternDelta) {}
        fn unmake_move(&self, _: &mut (), _: &PatternDelta, _: ()) {}
        fn evaluate(&self, _: &Position, _: &PatternState, _: &()) -> i32 {
            -10
        }
    }
    let p = position(&[110, 0, 111, 224]);
    let engine = AlphaBetaEngine::with_config(Flat, base());
    for cap in [1, 3000] {
        let mut state = SearchState::new(&p, &Flat);
        let mut context = engine.ab_context();
        context.profile = SearchProfile::baseline(ScoreContract::Pattern).with_qsearch_threes(true);
        let mut stats = SearchStatistics::default();
        let mut budget = SearchBudget::new(
            SearchLimits::new(1).with_max_nodes(cap),
            CancellationToken::new(),
        );
        let result = context.negamax::<true>(
            &mut state,
            0,
            -SEARCH_INFINITY,
            SEARCH_INFINITY,
            0,
            &mut SearchResources {
                budget: &mut budget,
                statistics: &mut stats,
                seldepth: &mut 0,
                pv: &mut PvTable::new(),
                heuristics: SearchHeuristics::default(),
                interior_proof: None,
                analysis: None,
            },
        );
        if cap == 1 {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().validity, BoundValidity::UNVERIFIED);
            assert!(stats.qsearch_three_edges > 0 && stats.qsearch_dependency_edges > 0);
        }
        assert!(budget.work_nodes() <= cap);
        assert_eq!(stats.tt_stores, 0);
        state.assert_consistent(&Flat);
        assert_eq!(state.position(), &p);
    }
}

#[test]
fn broad_teacher_can_find_a_best_move_outside_production_radius() {
    struct FarBest;
    impl Evaluator for FarBest {
        type State = ();
        type Undo = ();
        fn initialize(&self, _: &Position, _: &PatternState) {}
        fn make_move(&self, _: &mut (), _: &PatternDelta) {}
        fn unmake_move(&self, _: &mut (), _: &PatternDelta, _: ()) {}
        fn evaluate(&self, p: &Position, _: &PatternState, _: &()) -> i32 {
            -i32::from(p.cell(Move::from_index(0).unwrap()).is_some())
        }
        fn policy_score(&self, _: &Position, _: &PatternState, _: &(), at: Move) -> Option<i32> {
            Some(i32::from(at.index() == 0))
        }
    }
    let p = position(&[112]);
    let mut engine = AlphaBetaEngine::with_config(FarBest, base());
    let teacher = engine
        .analyze_root(
            &p,
            SearchLimits::new(1).with_max_nodes(1000),
            3,
            CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(teacher.completed_depth, 1);
    assert_eq!(teacher.candidates.len(), 224);
    let best = teacher
        .candidates
        .iter()
        .max_by_key(|candidate| candidate.score)
        .unwrap();
    assert_eq!(best.at.index(), 0);
    assert!(!crate::ProductionCandidateUniverse::new(&p).contains(best.at));
    let ordinary = engine.search(&p, SearchLimits::new(1));
    assert_eq!(ordinary.score, 0);
    engine.reconfigure(base().with_adaptive_root_candidates(true));
    engine.clear_transposition_table();
    let adaptive = engine.search(&p, SearchLimits::new(1));
    assert_eq!(adaptive.best_move, Some(best.at));
    assert_eq!(adaptive.score, 1);
    assert!(adaptive.statistics.root_candidates_added > 0);
    assert!(
        engine
            .table
            .probe(crate::zobrist::PositionKey::from_position(&p).value())
            .is_none()
    );
    assert_eq!(p, position(&[112]));
}

#[test]
fn root_teacher_keeps_common_horizon_and_does_not_touch_tt() {
    let p = position(&[112, 97, 128, 113]);
    let engine = AlphaBetaEngine::with_config(Ranked, base());
    let before = engine.transposition_table_statistics();
    for cap in [0, 1, 10, 1000] {
        let result = engine
            .analyze_root(
                &p,
                SearchLimits::new(3).with_max_nodes(cap),
                3,
                CancellationToken::new(),
            )
            .unwrap();
        assert!(result.work <= cap);
        for candidate in result.candidates {
            assert!(p.is_legal(candidate.at));
            assert_eq!(candidate.completed_depth, result.completed_depth);
            assert_eq!(candidate.score.is_some(), candidate.nominal_depth_valid);
        }
    }
    assert_eq!(engine.transposition_table_statistics(), before);
    let win = position(&[108, 0, 109, 2, 110, 4, 111, 6]);
    let analysis = engine
        .analyze_root(
            &win,
            SearchLimits::new(1).with_max_nodes(10000),
            1,
            CancellationToken::new(),
        )
        .unwrap();
    for at in Move::all().filter(|&at| win.would_win(at, win.side_to_move())) {
        assert!(
            analysis
                .candidates
                .iter()
                .any(|c| c.at == at && c.score == Some(MATE_SCORE - 1))
        );
    }
}

#[test]
fn public_policy_reduction_triggers_and_missing_policy_falls_back() {
    let p = position(&[112, 0, 224, 14, 210, 7, 217, 105]);
    let profile = SearchProfile::baseline(ScoreContract::Pattern).with_policy_lmr(true);
    let config = base()
        .with_search_profile(profile)
        .with_selectivity(SelectivityConfig {
            lmp: false,
            ..SelectivityConfig::BASELINE
        });
    let limits = SearchLimits::new(5).with_max_nodes(100_000);
    let result = AlphaBetaEngine::with_config(Ranked, config).search(&p, limits);
    assert!(
        result.statistics.policy_lmr_reductions > 0,
        "{:?}",
        result.statistics
    );
    assert!(result.statistics.work_nodes <= 100_000);
    let missing = AlphaBetaEngine::with_config(PatternEvaluator, config).search(&p, limits);
    assert_eq!(missing.statistics.policy_lmr_reductions, 0);
    let mismatch = config.with_search_profile(
        SearchProfile::baseline(ScoreContract::LinearV1).with_policy_lmr(true),
    );
    let mismatched = AlphaBetaEngine::with_config(Ranked, mismatch).search(&p, limits);
    assert_eq!(mismatched.statistics.policy_lmr_reductions, 0);
}

#[test]
fn excluded_search_ignores_normal_tt_and_restores_cancelled_state() {
    let p = position(&[112, 97, 128, 113]);
    let engine = AlphaBetaEngine::with_config(Ranked, base());
    let mut state = SearchState::new(&p, &Ranked);
    let at = state.candidates().iter().next().unwrap();
    engine.table.store_with_outcome(TtEntry::new(
        state.key().value(),
        50000,
        Some(at),
        2,
        Bound::Exact,
        0,
    ));
    let before = engine.table.probe(state.key().value());
    let mut context = engine.ab_context();
    context.domain = SearchDomain::Excluded { at, ply: 0 };
    context.selectivity = SelectivityConfig::OFF;
    for cap in [1, 10_000] {
        let mut budget = SearchBudget::new(
            SearchLimits::new(2).with_max_nodes(cap),
            CancellationToken::new(),
        );
        let mut pv = PvTable::new();
        let mut seldepth = 0;
        let mut stats = SearchStatistics::default();
        let result = context.negamax::<false>(
            &mut state,
            2,
            -1,
            1,
            0,
            &mut SearchResources {
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut stats,
                heuristics: SearchHeuristics::default(),
                interior_proof: None,
                analysis: None,
                budget: &mut budget,
            },
        );
        if cap == 1 {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().score, 0);
        }
        assert_eq!(state.position(), &p);
        state.assert_consistent(&Ranked);
        assert_eq!(engine.table.probe(state.key().value()), before);
        assert_eq!(stats.tt_probes + stats.tt_stores, 0);
    }
}

#[test]
fn public_probcut_is_bound_to_model_and_profile() {
    let p = position(&[112, 0, 224, 14, 210, 7, 217, 105]);
    let profile = SearchProfile::baseline(ScoreContract::Pattern);
    let bucket = ProbCutBucket {
        deep: 3,
        shallow: 1,
        phase: 0,
        slope_q16: 65536,
        min_shallow: -10_000_000,
        max_shallow: 10_000_000,
        intercept: 1000,
        tail: 0,
        training_samples: 64,
        heldout_samples: 32,
        heldout_false_cuts: 0,
    };
    // A deliberately biased fixture stresses the TT firewall; it is not an
    // empirical calibration and must never appear in match evidence.
    let calibration =
        ProbCutCalibration::new([7; 32], profile, SelectivityConfig::BASELINE, &[bucket]).unwrap();
    let limits = SearchLimits::new(4).with_max_nodes(100_000);
    let result =
        AlphaBetaEngine::with_config(Ranked, base().with_probcut(calibration)).search(&p, limits);
    assert!(
        result.statistics.probcut_attempts > 0,
        "{:?}",
        result.statistics
    );
    assert!(result.statistics.probcut_cutoffs > 0);
    let mismatch = AlphaBetaEngine::with_config(PatternEvaluator, base().with_probcut(calibration))
        .search(&p, limits);
    assert_eq!(mismatch.statistics.probcut_attempts, 0);
    assert!(
        ProbCutCalibration::new(
            [7; 32],
            profile,
            SelectivityConfig::BASELINE,
            &[ProbCutBucket {
                heldout_samples: 0,
                ..bucket
            }]
        )
        .is_err()
    );
}

#[test]
fn completed_observer_soft_stop_preserves_first_iteration() {
    struct StopAfterOne;
    impl SearchObserver for StopAfterOne {
        fn on_info(&mut self, _: SearchInfo) {}
        fn should_stop(&mut self) -> bool {
            true
        }
    }
    let result = AlphaBetaEngine::with_config(Ranked, base()).search_controlled(
        &Position::default(),
        SearchLimits::new(6),
        CancellationToken::new(),
        &mut StopAfterOne,
    );
    assert_eq!(result.completed_depth, 1);
    assert_eq!(result.termination, SearchTermination::Completed);
    assert_eq!(result.best_move, Some(Move::CENTER));
}

#[test]
fn public_interior_probes_actually_run() {
    let p = position(&[108, 107, 109, 0, 110, 2, 66, 4, 81, 6]);
    let config = base()
        .with_interior_vcf(crate::ProofLimits::new(5, 120), 1200)
        .with_interior_vct(crate::ProofLimits::new(5, 120), 1200);
    let result = AlphaBetaEngine::with_config(PatternEvaluator, config)
        .search(&p, SearchLimits::new(5).with_max_nodes(100_000));
    assert!(
        result.statistics.interior_proof.attempts > 0,
        "{:?}",
        result.statistics
    );
    assert!(
        result.statistics.interior_proof.vct_attempts > 0,
        "{:?}",
        result.statistics
    );
    assert!(result.statistics.interior_proof.work <= 2400);
}

#[test]
fn score_analysis_is_exact_only_when_completed_and_has_no_tt_side_effects() {
    let p = position(&[112, 0, 224, 14, 210, 7, 217, 105]);
    let mut engine = AlphaBetaEngine::with_config(Ranked, base());
    engine.reconfigure(base().with_selectivity(SelectivityConfig::OFF));
    let expected = engine.search(&p, SearchLimits::new(2)).score;
    let before = engine.transposition_table_statistics();
    let complete = engine
        .analyze_score(
            &p,
            SearchLimits::new(2).with_max_nodes(100000),
            CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(complete.score, Some(expected));
    assert_eq!(complete.completed_depth, 2);
    assert!(complete.quiet);
    assert_eq!(before, engine.transposition_table_statistics());
    let token = CancellationToken::new();
    token.cancel();
    let stopped = engine
        .analyze_score(&p, SearchLimits::new(2).with_max_nodes(100000), token)
        .unwrap();
    assert_eq!(stopped.score, None);
    assert_eq!(stopped.completed_depth, 0);
    assert_eq!(stopped.termination, SearchTermination::Cancelled);
}

#[test]
fn public_singular_extension_has_a_verified_alternative_search() {
    let mut parameters = crate::SearchParameters::BASELINE;
    parameters.futility = [0, 0];
    let profile = SearchProfile::new(ScoreContract::Pattern, parameters)
        .unwrap()
        .with_singular(true);
    let mut selection = SelectivityConfig::BASELINE;
    selection.futility = false;
    let mut attempts = 0;
    let mut extensions = 0;
    for moves in [
        &[112, 97, 128, 113][..],
        &[112, 0, 224, 14, 210, 7, 217, 105][..],
    ] {
        let p = position(moves);
        let mut engine = AlphaBetaEngine::with_config(
            PatternEvaluator,
            base()
                .with_search_profile(profile)
                .with_selectivity(selection),
        );
        for _ in 0..2 {
            let result = engine.search(&p, SearchLimits::new(8).with_max_nodes(30000));
            attempts += result.statistics.singular_attempts;
            extensions += result.statistics.singular_extensions;
            assert!(result.statistics.work_nodes <= 30000);
            eprintln!(
                "singular attempts={} extensions={} work={}",
                result.statistics.singular_attempts,
                result.statistics.singular_extensions,
                result.statistics.work_nodes
            );
        }
    }
    assert!(attempts > 0);
    assert!(extensions > 0);
}
