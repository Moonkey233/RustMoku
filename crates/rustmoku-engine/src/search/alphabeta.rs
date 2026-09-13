use super::*;

impl<'a, E: Evaluator> AbContext<'a, E> {
    pub(super) fn negamax<const PVS: bool>(
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
            if self.domain == SearchDomain::Normal && self.profile.qsearch_threes() {
                // Hint replies omit siblings; no ordinary nominal-depth authority.
                return Ok(NodeResult::unverified(
                    self.qsearch(state, alpha, beta, ply, 0, resources)?,
                ));
            }
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
        let excluded_here = match self.domain {
            SearchDomain::Excluded {
                at,
                ply: excluded_ply,
            } if excluded_ply == ply => Some(at),
            _ => None,
        };
        if excluded_here.is_none()
            && let Some((_, score)) = tactic.resolve(ply, resources.pv, resources.seldepth)
        {
            return Ok(NodeResult::complete(score));
        }
        let forced_block = if excluded_here.is_some() {
            None
        } else {
            tactic.forced_block()
        };
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
        let mut probe = self.probe_tt(state, depth, alpha, beta, ply, resources.statistics);
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
        // Narrow scout-only scheduler: no immediate forced response, at least
        // three remaining plies, <= 32 candidates, and an actual Four move.
        let proof_hint = if PVS
            && scout_node
            && forced_block.is_none()
            && depth >= 3
            && candidate_bits.iter().count() <= 96
            && !crate::vct::attacks(state.patterns(), side).is_empty()
        {
            if let Some(proof) = resources.interior_proof.as_mut() {
                proof.probe(
                    state,
                    resources.budget,
                    &mut resources.statistics.interior_proof,
                )?
            } else {
                None
            }
        } else {
            None
        };
        let quiet_for_experiments = [side, side.opponent()].into_iter().all(|stone| {
            state
                .patterns()
                .moves_at_least(stone, ThreatProfile::Three)
                .is_empty()
        });
        let selective_node = PVS
            && scout_node
            && forced_block.is_none()
            && !strong_threats
            && alpha.abs() < MATE_THRESHOLD
            && beta.abs() < MATE_THRESHOLD;
        if selective_node
            && self.domain == SearchDomain::Normal
            && (8..=190).contains(&state.position().move_count())
            && let Some(bucket) = self
                .probcut
                .and_then(|calibration| calibration.bucket(depth, state.position().move_count()))
            && let Some(result) = self.probcut_probe(state, bucket, beta, ply, resources)?
        {
            return Ok(result);
        }
        let research = self.profile.research();
        let improving_eligible = self.domain == SearchDomain::Normal
            && research.improving
            && forced_block.is_none()
            && !strong_threats
            && alpha.abs() < MATE_THRESHOLD
            && beta.abs() < MATE_THRESHOLD;
        let static_eval = if (selective_node && depth <= 3)
            || improving_eligible
            || (selective_node && research.null_move)
        {
            resources.statistics.static_evaluations += 1;
            let score = state.evaluate(self.evaluator);
            resources.heuristics.set_static_eval(ply, score);
            Some(score)
        } else {
            resources.heuristics.static_eval(ply)
        };
        let improving = improving_eligible && resources.heuristics.improving(ply);
        let margin_percent = if improving {
            i32::from(research.improving_margin_percent)
        } else {
            100
        };
        if selective_node
            && self.domain == SearchDomain::Normal
            && research.null_move
            && quiet_for_experiments
            && depth >= research.null_min_depth
            && self.evaluator.supports_analysis_turn()
            && usize::from(ply) < state.position().move_count()
            && static_eval.is_some_and(|score| {
                score.abs() < MATE_THRESHOLD
                    && score
                        >= beta.saturating_add(
                            self.profile
                                .margin(self.profile.parameters().reverse_futility, depth),
                        )
            })
            && let Some(result) = self.null_probe(state, depth, beta, ply, resources)?
        {
            return Ok(result);
        }
        if self.selectivity.reverse_futility && selective_node && depth <= 3 {
            resources.statistics.rfp_attempts += 1;
            if static_eval.is_some_and(|score| {
                score
                    - self
                        .profile
                        .margin(self.profile.parameters().reverse_futility, depth)
                        * margin_percent
                        / 100
                    >= beta
            }) {
                resources.statistics.rfp_cutoffs += 1;
                return Ok(NodeResult::unverified(
                    static_eval.expect("computed static eval"),
                ));
            }
        }
        if self.selectivity.razoring
            && selective_node
            && depth <= 2
            && static_eval.is_some_and(|score| {
                score + self.profile.margin(self.profile.parameters().razor, depth) < alpha
            })
        {
            resources.statistics.razor_attempts += 1;
            let score = self.qsearch(state, alpha, beta, ply, 0, resources)?;
            if score <= alpha {
                resources.statistics.razor_cutoffs += 1;
                return Ok(NodeResult::unverified(score));
            }
        }
        if PVS
            && self.domain == SearchDomain::Normal
            && research.iid
            && probe.best_move.is_none()
            && depth >= research.iid_min_depth
            && forced_block.is_none()
            && !strong_threats
        {
            probe.best_move = self.iid_move(state, depth, ply, resources)?;
        }
        let iir = PVS
            && self.selectivity.iir
            && scout_node
            && forced_block.is_none()
            && !strong_threats
            && probe.best_move.is_none()
            && depth >= self.profile.parameters().iir_min_depth;
        let searched_depth = depth - u8::from(iir);
        resources.statistics.iir_reductions += u64::from(iir);
        let mut moves = if let Some(excluded) = excluded_here {
            let mut moves = MoveList::new();
            for at in self.candidates(state).iter().filter(|&at| at != excluded) {
                moves.push(at);
            }
            moves
        } else if let Some(at) = forced_block {
            let mut moves = MoveList::new();
            moves.push(at);
            moves
        } else {
            self.candidates(state)
        };
        if moves.is_empty() {
            return Ok(if excluded_here.is_some() {
                NodeResult::unverified(0)
            } else {
                NodeResult::complete(0)
            });
        }
        let singular_move = if PVS
            && self.domain == SearchDomain::Normal
            && self.profile.singular()
            && !iir
            && forced_block.is_none()
            && resources.heuristics.extensions(ply) == 0
            && depth >= 5
            && let Some(entry) = probe.entry
            && entry.depth >= depth.saturating_sub(2)
            && matches!(entry.bound, Bound::Exact | Bound::Lower)
            && score_from_tt(entry.score, ply).abs() < MATE_THRESHOLD
            && let Some(at) = probe.best_move
            && moves.iter().any(|candidate| candidate != at)
        {
            self.verify_singular(state, entry, depth, ply, resources)?
                .then_some(at)
        } else {
            None
        };
        order_moves(
            side,
            state.patterns(),
            &mut moves,
            probe.best_move,
            &resources.heuristics,
            ply,
            |at| {
                if proof_hint == Some(at) {
                    Some(i32::from(i16::MAX))
                } else {
                    state.policy_score(self.evaluator, at)
                }
            },
        );
        let mut best_move = None;
        let mut best_score = -SEARCH_INFINITY;
        let (previous, two_back) = resources.heuristics.previous_moves(ply);
        let policy_ranks = (PVS
            && (self.profile.policy_lmr() || research.policy_pruning || research.lmr_v2)
            && scout_node
            && forced_block.is_none())
        .then(|| PolicyRanks::new(state, self.evaluator, &moves));
        let policy_protected = if research.policy_pruning {
            state.critical_dependencies()
        } else {
            crate::bitboard::BitBoard256::EMPTY
        };
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
            if research.policy_pruning
                && quiet_for_experiments
                && self.domain == SearchDomain::Normal
                && late_quiet
                && super::selectivity::policy_candidate_unprotected(at, policy_protected)
                && proof_hint != Some(at)
                && searched_depth <= research.policy_max_depth
                && policy_ranks.as_ref().is_some_and(|ranks| {
                    ranks.low_tail(
                        state.policy_score(self.evaluator, at),
                        research.policy_tail_percent,
                    )
                })
            {
                resources.statistics.policy_pruned_moves += 1;
                validity.upper = false;
                continue;
            }
            if self.selectivity.lmp
                && late_quiet
                && searched_depth <= 3
                && index
                    >= usize::from(
                        self.profile.parameters().lmp
                            [usize::from(searched_depth.saturating_sub(1).min(2))]
                            + u16::from(improving) * research.improving_lmp_bonus,
                    )
            {
                resources.statistics.lmp_pruned_moves += 1;
                validity.upper = false;
                continue;
            }
            if self.selectivity.futility
                && late_quiet
                && searched_depth <= 2
                && static_eval.is_some_and(|score| {
                    score
                        + self
                            .profile
                            .margin(self.profile.parameters().futility, searched_depth)
                            * margin_percent
                            / 100
                        <= alpha
                })
            {
                resources.statistics.futility_pruned_moves += 1;
                validity.upper = false;
                continue;
            }
            let extension = u8::from(
                singular_move == Some(at)
                    || (self.selectivity.threat_extension
                        && resources.heuristics.extensions(ply)
                            < self.profile.parameters().extension_budget
                        && threat_extension(
                            state.patterns().profile(at, side),
                            resources.heuristics.extensions(ply),
                        )),
            );
            resources.statistics.singular_extensions += u64::from(singular_move == Some(at));
            resources.statistics.threat_extensions +=
                u64::from(extension != 0 && singular_move != Some(at));
            let child_depth = searched_depth - 1 + extension;
            let mut reduction = if PVS
                && self.selectivity.lmr
                && scout_node
                && forced_block.is_none()
                && probe.best_move != Some(at)
                && alpha.abs() < MATE_THRESHOLD
                && proof_hint != Some(at)
                && extension == 0
            {
                if research.lmr_v2 {
                    if resources.heuristics.lmr_eligible(
                        searched_depth,
                        index,
                        side,
                        at,
                        ply,
                        previous,
                        two_back,
                        state.patterns(),
                    ) {
                        let base = super::selectivity::lmr_v2(
                            searched_depth,
                            index,
                            resources.heuristics.cut_node(ply),
                            improving,
                            research,
                        );
                        let history = resources
                            .heuristics
                            .contextual_score(side, at, previous, two_back);
                        let adjustment = resources
                            .heuristics
                            .lmr_history_adjustment(history, research.lmr_cut_bonus);
                        (i16::from(base) - adjustment).clamp(0, i16::from(child_depth)) as u8
                    } else {
                        0
                    }
                } else {
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
                }
            } else {
                0
            };
            if !research.lmr_v2 && reduction > 0 && improving {
                reduction = reduction.saturating_sub(research.improving_lmr_discount);
            }
            let policy_reduced = (self.profile.policy_lmr() || research.lmr_v2)
                && reduction > 0
                && reduction < child_depth
                && policy_ranks
                    .as_ref()
                    .is_some_and(|ranks| ranks.lower_half(state.policy_score(self.evaluator, at)));
            if policy_reduced {
                reduction += 1;
                resources.statistics.policy_lmr_reductions += 1;
            }

            reduction = reduction.min(child_depth);

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
                    resources.statistics.policy_lmr_researches += u64::from(policy_reduced);
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
            if policy_reduced && !reduced_fail_low && result.score <= alpha {
                resources.statistics.policy_lmr_failed_verifications += 1;
            }
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
}
