use super::*;

impl<'a, E: Evaluator> AbContext<'a, E> {
    pub(super) fn qsearch(
        &self,
        state: &mut SearchState<E>,
        alpha: i32,
        beta: i32,
        ply: u8,
        qply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<i32, Stopped> {
        self.qsearch_inner(
            state,
            alpha,
            beta,
            ply,
            QContext { qply, active: None },
            resources,
        )
    }

    pub(super) fn qsearch_inner(
        &self,
        state: &mut SearchState<E>,
        mut alpha: i32,
        beta: i32,
        ply: u8,
        context: QContext,
        resources: &mut SearchResources<'_>,
    ) -> Result<i32, Stopped> {
        let qply = context.qply;
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
        let extended = self.domain == SearchDomain::Normal && self.profile.qsearch_threes();
        let mut noisy = forcing_moves(patterns, side);
        if extended && qply == 0 {
            noisy = noisy.union(patterns.moves_at_least(side, ThreatProfile::OpenThree));
        }
        let replies = context
            .active
            .map_or(crate::bitboard::BitBoard256::EMPTY, |threat| {
                threat.reply_hints(patterns, side)
            });
        noisy = noisy.union(replies);
        let mut moves = MoveList::new();
        for at in noisy.iter() {
            moves.push(at);
        }
        order_moves(
            side,
            patterns,
            &mut moves,
            None,
            &resources.heuristics,
            ply,
            |at| state.policy_score(self.evaluator, at),
        );
        for at in moves.iter() {
            resources.statistics.qsearch_forcing_edges += 1;
            let active = if extended
                && qply == 0
                && state.patterns().profile(at, side) < ThreatProfile::Four
            {
                resources.statistics.qsearch_three_edges += 1;
                state.threat(at)
            } else {
                None
            };
            resources.statistics.qsearch_dependency_edges += u64::from(replies.test(at));
            resources.heuristics.set_child(
                ply + 1,
                at,
                false,
                resources.heuristics.extensions(ply),
            );
            let undo = state
                .make_move(at, self.evaluator)
                .expect("forcing frontier moves are legal");
            let child = self.qsearch_inner(
                state,
                -beta,
                -alpha,
                ply + 1,
                QContext {
                    qply: qply + 1,
                    active,
                },
                resources,
            );
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
}
