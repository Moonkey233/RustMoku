use super::*;

impl<'a, E: Evaluator> AbContext<'a, E> {
    pub(super) fn search_iteration(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        previous_score: i32,
        resources: &mut SearchResources<'_>,
    ) -> Result<RootSearchResult, Stopped> {
        let mut delta = if depth < 2 || previous_score.abs() >= MATE_THRESHOLD {
            2 * SEARCH_INFINITY
        } else {
            self.profile
                .raw_threshold(self.profile.parameters().aspiration)
                .max(1)
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
    pub(super) fn search_root<const PVS: bool>(
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
            self.candidates(state)
        };
        if forced_block.is_none() && self.adaptive_root_candidates {
            let mut bits = state.candidate_bits();
            let hints = crate::tactical::ThreatResolver::new(state.patterns(), side).hints();
            bits = bits.union(hints);
            // A single all-board scan at the root admits the strongest learned
            // policy point even outside radius two. No recursive scan/allocation.
            if let Some((_, at)) = crate::TeacherCandidateUniverse::moves(state.position())
                .filter_map(|at| {
                    state
                        .policy_score(self.evaluator, at)
                        .map(|score| ((score, std::cmp::Reverse(at)), at))
                })
                .max_by_key(|(rank, _)| *rank)
            {
                bits.set(at);
            }
            for at in bits.and_not(state.candidate_bits()).iter() {
                if state.position().is_legal(at) {
                    moves.push(at);
                    resources.statistics.root_candidates_added += 1;
                }
            }
        }
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            tt_move,
            &resources.heuristics,
            0,
            |at| state.policy_score(self.evaluator, at),
        );
        if self.root_rotation != 0 && !moves.is_empty() {
            let rotation = self.root_rotation % moves.as_slice().len();
            moves.as_mut_slice().rotate_left(rotation);
        }
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;
        let mut best_exact = false;
        let mut searched_quiets = MoveList::new();

        for (index, at) in moves.iter().enumerate() {
            // A negative heuristic score is not a forced loss. Preserve ordinary
            // fixed-horizon canonical ties; resistance is for mate-domain losses.
            let resistance = self.root_resistance && best_score <= -MATE_THRESHOLD && best_exact;
            let preferred = best_move.is_none_or(|current| {
                if resistance {
                    resistance_key(
                        side,
                        state.patterns(),
                        at,
                        state.policy_score(self.evaluator, at),
                    ) > resistance_key(
                        side,
                        state.patterns(),
                        current,
                        state.policy_score(self.evaluator, current),
                    )
                } else {
                    at < current
                }
            });
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
                    && preferred
                {
                    // Scout equality is only a bound. Resolve the candidate before
                    // using any secondary preference, including resistance.
                    resources.statistics.pvs_researches += 1;
                    if resistance {
                        resources.statistics.root_resistance_researches += 1;
                    }
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
                || (score == best_score
                    && preferred
                    && (!resistance || (result.validity.lower && result.validity.upper)))
            {
                if score == best_score && resistance {
                    resources.statistics.root_resistance_ties += 1;
                }
                best_score = score;
                best_exact = result.validity.lower
                    && result.validity.upper
                    && score > original_alpha
                    && score < beta;
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
        // Broader root-only candidate scores do not have the ordinary interior
        // candidate horizon. Never let that root exception leak through the TT.
        if !self.adaptive_root_candidates
            && validity.supports(classify_bound(best_score, original_alpha, beta))
        {
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
}
