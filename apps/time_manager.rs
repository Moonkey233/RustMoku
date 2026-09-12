//! Application clock policy, shared by Arena and Native. Pure elapsed-time
//! inputs make decisions testable without sleeps or a wall-clock dependency.
use rustmoku_core::Move;
use rustmoku_engine::{SearchInfo, SearchObserver};
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
        }
    }

    pub fn completed(&mut self, depth: u8, at: Option<Move>, score: i32, elapsed: Duration) {
        if depth == 0 || elapsed < self.elapsed {
            return;
        }
        let cost = elapsed.saturating_sub(self.elapsed);
        let score_change = self
            .previous_score
            .map_or(0, |previous| score.abs_diff(previous));
        self.stable = if self.previous_move == at && score_change <= 250 {
            self.stable.saturating_add(1)
        } else {
            0
        };
        let drop = self
            .previous_score
            .is_some_and(|previous| i64::from(previous) - i64::from(score) > 10_000);
        if let Some(mut soft) = self.soft {
            if drop {
                soft = soft.saturating_add(soft / 2);
            }
            if self.stable >= 3 {
                soft = soft.saturating_sub(soft / 4);
            }
            soft = self.ceiling.map_or(soft, |ceiling| soft.min(ceiling));
            let ratio = if self.iteration_cost.is_zero() {
                2
            } else {
                (cost.as_nanos() / self.iteration_cost.as_nanos()).clamp(2, 8) as u32
            };
            let next = cost.saturating_mul(ratio);
            self.stop = elapsed >= soft || elapsed.saturating_add(next) >= soft;
        }
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
        self.manager.completed(
            info.completed_depth,
            info.best_move,
            info.score,
            self.started.elapsed(),
        );
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
