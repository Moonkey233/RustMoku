//! Default-off, single-worker VCF ordering experiment. No score/TT authority.
use crate::{
    EngineConfig, Evaluator, ProofLimits,
    search_control::{SearchBudget, Stopped},
    search_state::SearchState,
    vcf::{VcfSolver, VcfStatus},
};
use rustmoku_core::Move;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteriorProofStatistics {
    pub attempts: u64,
    pub proven: u64,
    pub not_proven: u64,
    pub local_exhausted: u64,
    pub interrupted: u64,
    pub skipped: u64,
    pub cooldown_hits: u64,
    pub work: u64,
    pub certificate_work: u64,
    pub elapsed_nanos: u64,
}

pub(crate) struct InteriorProof {
    solver: VcfSolver,
    limits: ProofLimits,
    remaining: u64,
    // Collisions can only suppress an optional probe, never assert a fact.
    // The epoch is one public search, so model/window/depth changes cannot
    // revive a failed probe at every iterative-deepening revisit.
    seen: [Option<u64>; 256],
}

impl InteriorProof {
    pub(crate) fn new(config: EngineConfig) -> Option<Self> {
        let (limits, total) = config.interior_vcf();
        (config.threads() == 1 && limits.enabled() && total > 0).then(|| Self {
            solver: VcfSolver::new(),
            limits,
            remaining: total,
            seen: [None; 256],
        })
    }

    pub(crate) fn probe<E: Evaluator>(
        &mut self,
        state: &mut SearchState<E>,
        budget: &mut SearchBudget,
        stats: &mut InteriorProofStatistics,
    ) -> Result<Option<Move>, Stopped> {
        let key = state.key().value();
        let slot = key as usize & 255;
        if self.seen[slot] == Some(key) {
            stats.cooldown_hits += 1;
            return Ok(None);
        }
        let work = self.remaining.min(self.limits.max_nodes);
        if work < u64::from(self.limits.max_plies) + 2 {
            stats.skipped += 1;
            return Ok(None);
        }
        self.seen[slot] = Some(key);
        stats.attempts += 1;
        let before = budget.work_nodes();
        let start = std::time::Instant::now();
        let (status, hint) =
            state.vcf_ordering_hint(&mut self.solver, self.limits.max_plies, work, budget);
        let spent = budget.work_nodes() - before;
        debug_assert!(spent <= work);
        self.remaining -= spent;
        stats.work += spent;
        stats.certificate_work += spent - self.solver.statistics().nodes;
        stats.elapsed_nanos = stats
            .elapsed_nanos
            .saturating_add(start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
        match status {
            VcfStatus::ProvenWin { .. } => stats.proven += 1,
            VcfStatus::NotProven => stats.not_proven += 1,
            VcfStatus::BudgetExceeded => stats.local_exhausted += 1,
            VcfStatus::Interrupted => {
                stats.interrupted += 1;
                return Err(Stopped);
            }
        }
        budget.poll()?;
        Ok(hint.filter(|&at| state.position().is_legal(at)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancellationToken, PatternEvaluator, SearchLimits};
    use rustmoku_core::Position;

    fn fixture() -> Position {
        let mut p = Position::default();
        for index in [108, 107, 109, 0, 110, 2, 66, 4, 81, 6] {
            p.make_move(Move::from_index(index).unwrap()).unwrap();
        }
        p
    }

    #[test]
    fn hint_restores_sidecars_and_caps_every_visit() {
        let position = fixture();
        for cap in [0, 1, 7, 32, 100, 10_000] {
            let config = EngineConfig::new(0).with_interior_vcf(ProofLimits::new(5, cap), cap);
            let Some(mut proof) = InteriorProof::new(config) else {
                continue;
            };
            let mut state = SearchState::new(&position, &PatternEvaluator);
            let key = state.key();
            let value = state.evaluate(&PatternEvaluator);
            let mut budget = SearchBudget::default();
            let mut stats = InteriorProofStatistics::default();
            let hint = proof.probe(&mut state, &mut budget, &mut stats).unwrap();
            assert!(stats.work <= cap);
            assert_eq!(stats.work, budget.work_nodes());
            assert_eq!(state.position(), &position);
            assert_eq!(state.key(), key);
            assert_eq!(state.evaluate(&PatternEvaluator), value);
            for at in Move::all() {
                assert_eq!(state.policy_score(&PatternEvaluator, at), None);
            }
            if cap == 10_000 {
                assert!(hint.is_some());
                assert_eq!(stats.proven, 1);
            }
            let work = stats.work;
            assert!(
                proof
                    .probe(&mut state, &mut budget, &mut stats)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(stats.work, work);
        }
    }

    #[test]
    fn global_stops_restore_board_and_smp_disables_probe() {
        let config = EngineConfig::new(0).with_interior_vcf(ProofLimits::new(5, 10_000), 10_000);
        assert!(InteriorProof::new(EngineConfig::default()).is_none());
        assert!(InteriorProof::new(config.with_threads(2)).is_none());
        for cap in 0..16 {
            let position = fixture();
            let mut state = SearchState::new(&position, &PatternEvaluator);
            let token = CancellationToken::new();
            if cap == 15 {
                token.cancel();
            }
            let mut budget = SearchBudget::new(SearchLimits::new(6).with_max_nodes(cap), token);
            let mut stats = InteriorProofStatistics::default();
            let mut proof = InteriorProof::new(config).unwrap();
            let result = proof.probe(&mut state, &mut budget, &mut stats);
            assert_eq!(state.position(), &position);
            assert!(budget.work_nodes() <= cap);
            if cap == 0 || cap == 15 {
                assert!(result.is_err());
            }
        }
    }

    #[test]
    fn ablation_off_preserves_exact_tactics_and_has_no_selective_events() {
        use crate::{AlphaBetaEngine, SearchEngine, SelectivityConfig};
        let mut p = Position::default();
        for index in [112, 97, 128, 113] {
            p.make_move(Move::from_index(index).unwrap()).unwrap();
        }
        let config = EngineConfig::new(0)
            .with_selectivity(SelectivityConfig::OFF)
            .with_vcf_limits(0, 0)
            .with_vct_limits(0, 0);
        let result =
            AlphaBetaEngine::with_config(PatternEvaluator, config).search(&p, SearchLimits::new(4));
        let s = result.statistics;
        assert_eq!(
            s.rfp_attempts
                + s.razor_attempts
                + s.lmp_pruned_moves
                + s.futility_pruned_moves
                + s.lmr_reductions
                + s.iir_reductions
                + s.threat_extensions,
            0
        );
        let mut p = Position::default();
        for index in [109, 0, 110, 2, 112, 4, 113, 6] {
            p.make_move(Move::from_index(index).unwrap()).unwrap();
        }
        let on = AlphaBetaEngine::with_config(
            PatternEvaluator,
            config.with_selectivity(SelectivityConfig::BASELINE),
        )
        .search(&p, SearchLimits::new(4));
        let off =
            AlphaBetaEngine::with_config(PatternEvaluator, config).search(&p, SearchLimits::new(4));
        assert_eq!(on.best_move, off.best_move);
        assert_eq!(on.score, off.score);
    }
}
