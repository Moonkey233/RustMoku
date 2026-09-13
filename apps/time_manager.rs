//! Application clock policy, shared by Arena and Native. Pure elapsed-time
//! inputs make decisions testable without sleeps or a wall-clock dependency.
use rustmoku_core::Move;
use rustmoku_engine::{
    ResearchParameters, ScoreContract, SearchInfo, SearchObserver, SearchProfile,
};
use std::time::{Duration, Instant};

pub struct TimeManager {
    soft: Option<Duration>,
    ceiling: Option<Duration>,
    previous_move: Option<Move>,
    previous_score: Option<i32>,
    stable: u8,
    elapsed: Duration,
    iteration_cost: Duration,
    stop: bool,
    profile: SearchProfile,
    growth_q8: u32,
    previous_pv: Vec<Move>,
    pv_stable: bool,
    previous_mate: Option<bool>,
    phase_stones: usize,
}

impl TimeManager {
    pub fn new(remaining: Option<Duration>, increment: Duration, turn: Option<Duration>) -> Self {
        let hard = match (remaining, turn) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let ceiling = hard.map(|time| time.saturating_sub(time / 10));
        let base = remaining
            .map(|time| (time / 20).saturating_add(increment / 4 * 3))
            .or(turn);
        let soft = match (base, ceiling) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Self {
            soft,
            ceiling,
            previous_move: None,
            previous_score: None,
            stable: 0,
            elapsed: Duration::ZERO,
            iteration_cost: Duration::ZERO,
            stop: false,
            profile: SearchProfile::baseline(ScoreContract::Pattern),
            growth_q8: u32::from(ResearchParameters::OFF.growth_initial_q8),
            previous_pv: Vec::new(),
            pv_stable: true,
            previous_mate: None,
            phase_stones: 0,
        }
    }

    pub fn with_profile(mut self, profile: SearchProfile, stones: usize) -> Self {
        self.profile = profile;
        self.growth_q8 = u32::from(profile.research().growth_initial_q8);
        self.phase_stones = stones;
        self
    }
    pub fn completed_info(&mut self, info: &SearchInfo, elapsed: Duration) {
        let prefix = self
            .previous_pv
            .len()
            .min(info.principal_variation.len())
            .min(4);
        self.pv_stable =
            prefix > 0 && self.previous_pv[..prefix] == info.principal_variation[..prefix];
        self.completed(info.completed_depth, info.best_move, info.score, elapsed);
        self.previous_pv.clone_from(&info.principal_variation);
        if info.proof.is_some() {
            self.stop = true;
        }
    }

    pub fn completed(&mut self, depth: u8, at: Option<Move>, score: i32, elapsed: Duration) {
        if depth == 0 || elapsed < self.elapsed {
            return;
        }
        let cost = elapsed.saturating_sub(self.elapsed);
        let parameters = self.profile.research();
        let mate = ScoreContract::is_mate_score(score);
        let mate_transition = self.previous_mate.is_some_and(|old| old != mate);
        let score = self.profile.reference_score(score);
        let score_change = self
            .previous_score
            .map_or(0, |previous| score.abs_diff(previous));
        self.stable = if self.previous_move == at
            && self.pv_stable
            && !mate_transition
            && score_change <= u32::from(parameters.time_stability)
        {
            self.stable.saturating_add(1)
        } else {
            0
        };
        let drop = self.previous_score.is_some_and(|previous| {
            i64::from(previous) - i64::from(score) > i64::from(parameters.time_drop)
        });
        if let Some(mut soft) = self.soft {
            if drop || mate_transition {
                soft = soft.saturating_add(soft / 2);
            }
            if self.stable
                >= if (20..80).contains(&self.phase_stones) {
                    4
                } else {
                    3
                }
            {
                soft = soft.saturating_sub(soft / 4);
            }
            soft = self.ceiling.map_or(soft, |ceiling| soft.min(ceiling));
            if !self.iteration_cost.is_zero() {
                let observed =
                    (cost.as_nanos().saturating_mul(256) / self.iteration_cost.as_nanos()).clamp(
                        u128::from(parameters.growth_min_q8),
                        u128::from(parameters.growth_max_q8),
                    ) as u32;
                let old_weight = u32::from(parameters.growth_ema_weight);
                self.growth_q8 = (old_weight * self.growth_q8 + observed) / (old_weight + 1);
            }
            let next = cost.saturating_mul(self.growth_q8) / 256;
            self.stop = elapsed >= soft || elapsed.saturating_add(next) >= soft;
        }
        self.previous_mate = Some(mate);
        self.previous_move = at;
        self.previous_score = Some(score);
        self.elapsed = elapsed;
        self.iteration_cost = cost;
    }

    pub fn should_stop(&self) -> bool {
        self.stop
    }
}

pub struct ManagedObserver<F> {
    manager: TimeManager,
    started: Instant,
    callback: F,
}
impl<F> ManagedObserver<F> {
    pub fn new(manager: TimeManager, callback: F) -> Self {
        Self {
            manager,
            started: Instant::now(),
            callback,
        }
    }
}
impl<F: FnMut(SearchInfo)> SearchObserver for ManagedObserver<F> {
    fn on_info(&mut self, info: SearchInfo) {
        self.manager.completed_info(&info, self.started.elapsed());
        (self.callback)(info);
    }
    fn should_stop(&mut self) -> bool {
        self.manager.should_stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_small_increment_and_overflow_budgets() {
        for budget in [Duration::ZERO, Duration::from_nanos(1), Duration::MAX] {
            let mut manager = TimeManager::new(Some(budget), Duration::MAX, Some(budget));
            assert!(!manager.should_stop());
            manager.completed(0, Some(Move::CENTER), 0, Duration::MAX);
            assert!(!manager.should_stop());
            manager.completed(1, Some(Move::CENTER), 0, budget);
            assert!(manager.should_stop());
        }
        let mut manager =
            TimeManager::new(Some(Duration::from_secs(20)), Duration::from_secs(1), None);
        manager.completed(1, Some(Move::CENTER), 0, Duration::from_millis(100));
        assert!(!manager.should_stop());
    }
    #[test]
    fn severe_drop_extends_soft_budget_but_never_hard_ceiling() {
        let mut steady = TimeManager::new(Some(Duration::from_secs(20)), Duration::ZERO, None);
        let mut drop = TimeManager::new(Some(Duration::from_secs(20)), Duration::ZERO, None);
        for manager in [&mut steady, &mut drop] {
            manager.completed(1, Some(Move::CENTER), 20_000, Duration::from_millis(100));
        }
        steady.completed(2, Some(Move::CENTER), 20_000, Duration::from_millis(430));
        drop.completed(2, Some(Move::CENTER), -20_000, Duration::from_millis(430));
        assert!(steady.should_stop());
        assert!(!drop.should_stop());
        drop.completed(3, Some(Move::CENTER), i32::MIN, Duration::from_secs(20));
        assert!(drop.should_stop());
    }
}

#[cfg(test)]
mod v2_tests {
    use super::*;
    #[test]
    fn empirical_growth_can_fall_below_two_and_scaled_scores_match() {
        let mut a = TimeManager::new(Some(Duration::from_secs(100)), Duration::ZERO, None);
        let mut b = TimeManager::new(Some(Duration::from_secs(100)), Duration::ZERO, None)
            .with_profile(
                SearchProfile::baseline(ScoreContract::RationalV2 { scale: 500 }),
                0,
            );
        for depth in 1..=6 {
            let elapsed = Duration::from_millis(u64::from(depth) * 100);
            a.completed(depth, Some(Move::CENTER), 4000, elapsed);
            b.completed(depth, Some(Move::CENTER), 200, elapsed);
            assert_eq!(a.stable, b.stable);
            assert_eq!(a.should_stop(), b.should_stop());
        }
        assert!(a.growth_q8 < 384);
        assert!(a.growth_q8 >= 256);
        a.completed(7, Some(Move::CENTER), i32::MAX, Duration::from_millis(700));
        assert_eq!(a.stable, 0);
    }
}
