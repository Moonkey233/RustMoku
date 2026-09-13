use super::*;

impl<E: Evaluator> AlphaBetaEngine<E> {
    /// A full-window search over the normal candidate universe. Research probes,
    /// selectivity, proof shortcuts, and normal TT reads/writes are disabled.
    pub fn analyze_score(
        &self,
        position: &Position,
        limits: SearchLimits,
        cancellation: CancellationToken,
    ) -> Result<ScoreAnalysis, &'static str> {
        if limits.max_depth == 0 || limits.max_nodes.is_none() {
            return Err("score analysis requires positive depth and an explicit work cap");
        }
        let mut state = SearchState::new(position, &self.evaluator);
        let side = position.side_to_move();
        let quiet = [side, side.opponent()].into_iter().all(|stone| {
            state
                .patterns()
                .moves_at_least(stone, ThreatProfile::OpenThree)
                .is_empty()
        });
        let mut budget = SearchBudget::new(limits, cancellation);
        let mut statistics = SearchStatistics::default();
        let mut pv = PvTable::new();
        let mut seldepth = 0;
        let mut context = self.ab_context();
        context.domain = SearchDomain::Analysis;
        context.profile = self
            .config
            .effective_profile(self.evaluator.score_contract());
        context.selectivity = crate::SelectivityConfig::OFF;
        let result = context.negamax::<false>(
            &mut state,
            limits.max_depth,
            -SEARCH_INFINITY,
            SEARCH_INFINITY,
            0,
            &mut SearchResources {
                seldepth: &mut seldepth,
                pv: &mut pv,
                statistics: &mut statistics,
                heuristics: SearchHeuristics::default(),
                interior_proof: None,
                analysis: None,
                budget: &mut budget,
            },
        );
        let score = match result {
            Ok(value) if value.validity.supports(Bound::Exact) && budget.poll().is_ok() => {
                Some(value.score)
            }
            Ok(_) | Err(_) => None,
        };
        Ok(ScoreAnalysis {
            score,
            requested_depth: limits.max_depth,
            completed_depth: if score.is_some() { limits.max_depth } else { 0 },
            termination: budget.termination(),
            work: budget.work_nodes(),
            quiet,
        })
    }

    /// Bounded teacher interface, independent of the normal public search result.
    /// Every legal root move is compared at a common completed horizon. `top_k`
    /// controls the production ablation only; callers rank/truncate broad output.
    pub fn analyze_root(
        &self,
        position: &Position,
        limits: SearchLimits,
        top_k: usize,
        cancellation: CancellationToken,
    ) -> Result<RootAnalysis, &'static str> {
        self.analyze_root_with_candidates(
            position,
            limits,
            top_k,
            crate::TeacherCandidates::Practical,
            cancellation,
        )
    }

    /// Explicit candidate universe for offline recall comparisons and ablations.
    pub fn analyze_root_with_candidates(
        &self,
        position: &Position,
        limits: SearchLimits,
        top_k: usize,
        universe: crate::TeacherCandidates,
        cancellation: CancellationToken,
    ) -> Result<RootAnalysis, &'static str> {
        if !(1..=16).contains(&top_k) || limits.max_depth == 0 || limits.max_nodes.is_none() {
            return Err(
                "root analysis requires top-k 1..16, positive depth and an explicit work cap",
            );
        }
        let mut state = SearchState::new(position, &self.evaluator);
        let mut budget = SearchBudget::new(limits, cancellation);
        let mut statistics = SearchStatistics::default();
        let mut pv = PvTable::new();
        let mut seldepth = 0;
        let mut moves = state.candidates();
        let side = position.side_to_move();
        let protected = state
            .patterns()
            .winning_moves(side)
            .union(state.patterns().winning_moves(side.opponent()));
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            None,
            &SearchHeuristics::default(),
            0,
            |at| state.policy_score(&self.evaluator, at),
        );
        let mut selected = crate::bitboard::BitBoard256::EMPTY;
        for at in moves.iter().take(top_k) {
            selected.set(at);
        }
        selected = selected.union(protected);
        if universe != crate::TeacherCandidates::ProductionTopK {
            selected = crate::bitboard::BitBoard256::EMPTY;
            for at in crate::TeacherCandidateUniverse::moves(position) {
                selected.set(at);
            }
        }
        let mut candidates: Vec<_> = selected
            .iter()
            .filter(|&at| position.is_legal(at))
            .map(|at| RootCandidate {
                at,
                score: None,
                bound: CandidateBound::Unknown,
                completed_depth: 0,
                nominal_depth_valid: false,
                source: SearchOrigin::Analysis,
                termination: SearchTermination::Completed,
                work: 0,
            })
            .collect();
        let mut layer = vec![None; candidates.len()];
        let mut context = self.ab_context();
        context.domain = if universe == crate::TeacherCandidates::AllLegal {
            SearchDomain::Teacher
        } else {
            SearchDomain::Analysis
        };
        context.profile = self
            .config
            .effective_profile(self.evaluator.score_contract());
        context.selectivity = crate::SelectivityConfig::OFF;
        let mut completed_depth = 0;
        // One set of large continuation tables for the entire teacher request.
        // History is ordering-only in this nonselective, TT-free analysis domain.
        let mut resources = SearchResources {
            seldepth: &mut seldepth,
            pv: &mut pv,
            statistics: &mut statistics,
            heuristics: SearchHeuristics::default(),
            interior_proof: None,
            analysis: None,
            budget: &mut budget,
        };
        if !candidates.is_empty() {
            'depth: for depth in 1..=limits.max_depth {
                for (i, candidate) in candidates.iter_mut().enumerate() {
                    if resources.budget.poll().is_err() {
                        break 'depth;
                    }
                    let before = resources.budget.work_nodes();
                    resources.heuristics.begin_root();
                    resources.heuristics.set_child(1, candidate.at, false, 0);
                    let undo = state
                        .make_move(candidate.at, &self.evaluator)
                        .expect("checked root candidate");
                    let result = context.negamax::<false>(
                        &mut state,
                        depth - 1,
                        -SEARCH_INFINITY,
                        SEARCH_INFINITY,
                        1,
                        &mut resources,
                    );
                    state.unmake_move(undo, &self.evaluator);
                    candidate.work += resources.budget.work_nodes() - before;
                    let Ok(result) = result else {
                        break 'depth;
                    };
                    if resources.budget.poll().is_err() {
                        break 'depth;
                    }
                    let result = -result;
                    if !result.validity.supports(Bound::Exact) {
                        return Err("full-window root analysis returned an unverified score");
                    }
                    layer[i] = Some(result.score);
                }
                for (candidate, score) in candidates.iter_mut().zip(&layer) {
                    candidate.score = *score;
                    candidate.bound = CandidateBound::DomainExact;
                    candidate.completed_depth = depth;
                    candidate.nominal_depth_valid = true;
                    candidate.source = SearchOrigin::AlphaBeta;
                }
                completed_depth = depth;
            }
        }
        let termination = budget.termination();
        for candidate in &mut candidates {
            candidate.termination = termination;
        }
        Ok(RootAnalysis {
            universe,
            score_contract: self.evaluator.score_contract(),
            side_to_move: side,
            candidates,
            completed_depth,
            termination,
            work: budget.work_nodes(),
        })
    }
}
