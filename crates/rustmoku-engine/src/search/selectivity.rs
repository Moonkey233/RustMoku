use super::*;

impl<'a, E: Evaluator> AbContext<'a, E> {
    pub(super) fn probcut_probe(
        &self,
        state: &mut SearchState<E>,
        bucket: crate::ProbCutBucket,
        beta: i32,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<Option<NodeResult>, Stopped> {
        let Some(mut scratch) = resources.analysis.take() else {
            return Ok(None);
        };
        let mut context = Self::new(self.evaluator, self.table, self.generation, 0);
        context.domain = SearchDomain::Analysis;
        context.selectivity = crate::SelectivityConfig::OFF;
        resources.statistics.probcut_attempts += 1;
        let before = resources.budget.work_nodes();
        let (result, heuristics) = {
            let mut isolated = SearchResources {
                seldepth: &mut scratch.seldepth,
                pv: &mut scratch.pv,
                statistics: resources.statistics,
                heuristics: scratch
                    .heuristics
                    .take()
                    .expect("exclusive analysis scratch"),
                interior_proof: None,
                analysis: None,
                budget: resources.budget,
            };
            let result = context.negamax::<false>(
                state,
                bucket.shallow,
                -SEARCH_INFINITY,
                SEARCH_INFINITY,
                ply,
                &mut isolated,
            );
            (result, isolated.heuristics)
        };
        scratch.heuristics = Some(heuristics);
        resources.analysis = Some(scratch);
        resources.statistics.probcut_work += resources.budget.work_nodes() - before;
        let result = result?;
        if !result.validity.supports(Bound::Exact) || result.score.abs() >= MATE_THRESHOLD {
            resources.statistics.probcut_unqualified += 1;
            return Ok(None);
        }
        if (bucket.min_shallow..=bucket.max_shallow).contains(&result.score)
            && bucket.lower_prediction(result.score) >= i64::from(beta)
        {
            resources.statistics.probcut_cutoffs += 1;
            // A regression fit is not a nominal-depth legal witness. Neither
            // this node nor an ancestor may promote it to an ordinary bound.
            return Ok(Some(NodeResult::unverified(beta)));
        }
        Ok(None)
    }

    pub(super) fn verify_singular(
        &self,
        state: &mut SearchState<E>,
        entry: TtEntry,
        depth: u8,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<bool, Stopped> {
        let Some(mut scratch) = resources.analysis.take() else {
            return Ok(false);
        };
        let at = entry
            .best_move()
            .expect("singular entry move was validated");
        let threshold = score_from_tt(entry.score, ply)
            - self
                .profile
                .margin(self.profile.parameters().futility, depth);
        let mut context = Self::new(self.evaluator, self.table, self.generation, 0);
        context.domain = SearchDomain::Excluded { at, ply };
        context.selectivity = crate::SelectivityConfig::OFF;
        resources.statistics.singular_attempts += 1;
        let before = resources.budget.work_nodes();
        let (result, heuristics) = {
            let mut isolated = SearchResources {
                seldepth: &mut scratch.seldepth,
                pv: &mut scratch.pv,
                statistics: resources.statistics,
                heuristics: scratch
                    .heuristics
                    .take()
                    .expect("scratch returned after every probe"),
                interior_proof: None,
                analysis: None,
                budget: resources.budget,
            };
            let result = context.negamax::<false>(
                state,
                (depth - 1) / 2,
                threshold - 1,
                threshold,
                ply,
                &mut isolated,
            );
            (result, isolated.heuristics)
        };
        scratch.heuristics = Some(heuristics);
        resources.analysis = Some(scratch);
        resources.statistics.singular_work += resources.budget.work_nodes() - before;
        if !result.as_ref().is_ok_and(|result| result.validity.upper) {
            resources.statistics.singular_incomplete += 1;
        }
        let result = result?;
        Ok(result.score < threshold && result.validity.upper)
    }
}

// Q8 piecewise-linear log2; precomputed once by const evaluation, no floating
// point, allocation or logarithm in recursion. Product gives a smooth depth/rank surface.
const LOG2_Q8: [u16; 256] = {
    let mut values = [0; 256];
    let mut n = 2usize;
    while n < 256 {
        let exponent = n.ilog2();
        let base = 1usize << exponent;
        values[n] = (exponent as usize * 256 + (n - base) * 256 / base) as u16;
        n += 1;
    }
    values
};
pub(super) fn lmr_v2(
    depth: u8,
    index: usize,
    cut: bool,
    improving: bool,
    p: crate::ResearchParameters,
) -> u8 {
    let product = u32::from(LOG2_Q8[usize::from(depth)]) * u32::from(LOG2_Q8[(index + 1).min(255)]);
    let base = product / (65536 * u32::from(p.lmr_divisor));
    let reduction = base + u32::from(cut) * u32::from(p.lmr_cut_bonus);
    reduction
        .saturating_sub(u32::from(improving) * u32::from(p.improving_lmr_discount))
        .min(u32::from(depth.saturating_sub(1))) as u8
}
impl<'a, E: Evaluator> AbContext<'a, E> {
    pub(super) fn iid_move(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<Option<Move>, Stopped> {
        let Some(mut scratch) = resources.analysis.take() else {
            return Ok(None);
        };
        let mut context = Self::new(self.evaluator, self.table, self.generation, 0);
        context.domain = SearchDomain::Analysis;
        context.profile = self.profile;
        context.selectivity = crate::SelectivityConfig::OFF;
        resources.statistics.iid_attempts += 1;
        let before = resources.budget.work_nodes();
        let (result, heuristics) = {
            let mut isolated = SearchResources {
                seldepth: &mut scratch.seldepth,
                pv: &mut scratch.pv,
                statistics: resources.statistics,
                heuristics: scratch.heuristics.take().expect("exclusive IID scratch"),
                interior_proof: None,
                analysis: None,
                budget: resources.budget,
            };
            isolated.heuristics.begin_root();
            let result = context.negamax::<false>(
                state,
                depth - self.profile.research().iid_reduction,
                -SEARCH_INFINITY,
                SEARCH_INFINITY,
                ply,
                &mut isolated,
            );
            (result, isolated.heuristics)
        };
        let at = scratch
            .pv
            .line(ply)
            .first()
            .copied()
            .filter(|&at| state.position().is_legal(at));
        scratch.heuristics = Some(heuristics);
        resources.analysis = Some(scratch);
        resources.statistics.iid_work += resources.budget.work_nodes() - before;
        result?;
        // No probe score crosses this boundary; the normal search still verifies its own depth.
        Ok(at)
    }
}

impl<'a, E: Evaluator> AbContext<'a, E> {
    pub(super) fn null_probe(
        &self,
        state: &mut SearchState<E>,
        depth: u8,
        beta: i32,
        ply: u8,
        resources: &mut SearchResources<'_>,
    ) -> Result<Option<NodeResult>, Stopped> {
        let Some(mut scratch) = resources.analysis.take() else {
            return Ok(None);
        };
        let mut context = Self::new(self.evaluator, self.table, self.generation, 0);
        context.domain = SearchDomain::Null;
        context.profile = self.profile;
        context.selectivity = crate::SelectivityConfig::OFF;
        let before = resources.budget.work_nodes();
        let (result, heuristics) = {
            let mut isolated = SearchResources {
                seldepth: &mut scratch.seldepth,
                pv: &mut scratch.pv,
                statistics: resources.statistics,
                heuristics: scratch.heuristics.take().expect("exclusive null scratch"),
                interior_proof: None,
                analysis: None,
                budget: resources.budget,
            };
            let result = (|| {
                let Some(undo) = state.begin_null(self.evaluator) else {
                    return Ok(None);
                };
                isolated.statistics.null_attempts += 1;
                isolated.heuristics.begin_root();
                let hypothetical = context.negamax::<false>(
                    state,
                    depth - self.profile.research().null_reduction,
                    -beta,
                    -beta + 1,
                    ply + 1,
                    &mut isolated,
                );
                state.end_null(undo);
                let hypothetical = hypothetical?;
                if -hypothetical.score < beta || hypothetical.score.abs() >= MATE_THRESHOLD {
                    return Ok(None);
                }
                // Real position, full nominal depth, no null/TT/selective shortcuts.
                // Even this verified pass-triggered result is not published as a TT fact.
                context.domain = SearchDomain::Analysis;
                isolated.statistics.null_verifications += 1;
                isolated.heuristics.begin_root();
                let verified =
                    context.negamax::<false>(state, depth, beta - 1, beta, ply, &mut isolated)?;
                if verified.score >= beta
                    && verified.score.abs() < MATE_THRESHOLD
                    && verified.validity.lower
                {
                    isolated.statistics.null_cutoffs += 1;
                    Ok(Some(NodeResult::unverified(verified.score)))
                } else {
                    Ok(None)
                }
            })();
            (result, isolated.heuristics)
        };
        scratch.heuristics = Some(heuristics);
        resources.analysis = Some(scratch);
        resources.statistics.null_work += resources.budget.work_nodes() - before;
        result
    }
}
