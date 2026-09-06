//! Shared per-public-search lifecycle and worker-local accounting.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::SearchLimits;

/// One-way cross-thread cancellation. Use a fresh token for each request.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchTermination {
    #[default]
    Completed,
    NodeLimit,
    TimeLimit,
    Cancelled,
}

/// An interruption is never a score, bound, or tactical disproof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stopped;

pub(crate) struct ProofResources<'a> {
    pub(crate) pv: &'a mut crate::principal_variation::PvTable,
    pub(crate) budget: &'a mut SearchBudget,
}

const STOP_COMPLETED: u8 = 0;
const STOP_NODE_LIMIT: u8 = 1;
const STOP_TIME_LIMIT: u8 = 2;
const STOP_CANCELLED: u8 = 3;
const TEAM_DONE: u8 = 0x80;
const STOP_MASK: u8 = 0x7f;

/// Mutable lifecycle state shared by the coordinator and all Alpha-Beta
/// workers. The node counter is touched atomically only when the caller asked
/// for a global node cap; uncapped searches retain local hot-path counters.
pub(crate) struct SharedSearchControl {
    start: Instant,
    move_time: Option<Duration>,
    max_nodes: Option<u64>,
    admitted_nodes: AtomicU64,
    /// Public stop code plus the internal team-done bit. Keeping these in one
    /// atomic makes the race between a helper's final poll and principal
    /// completion explicit: exactly one of stop or team completion wins.
    state: AtomicU8,
    cancellation: CancellationToken,
}

impl SharedSearchControl {
    fn new(limits: SearchLimits, cancellation: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            move_time: limits.move_time,
            max_nodes: limits.max_nodes,
            admitted_nodes: AtomicU64::new(0),
            state: AtomicU8::new(STOP_COMPLETED),
            cancellation,
        })
    }

    fn try_admit_node(&self) -> bool {
        let Some(max_nodes) = self.max_nodes else {
            return true;
        };
        let mut current = self.admitted_nodes.load(Ordering::Relaxed);
        loop {
            if current >= max_nodes {
                self.note_stop(SearchTermination::NodeLimit);
                return false;
            }
            match self.admitted_nodes.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn note_stop(&self, reason: SearchTermination) {
        let value = stop_code(reason);
        // A successful transition is the linearization point for the public
        // stop. If the principal has already claimed TEAM_DONE, this fails
        // and the helper's stop is only an internal shutdown.
        let _ =
            self.state
                .compare_exchange(STOP_COMPLETED, value, Ordering::AcqRel, Ordering::Acquire);
    }

    fn poll(&self) -> Result<(), Stopped> {
        let state = self.state.load(Ordering::Acquire);
        if state & STOP_MASK != STOP_COMPLETED {
            return Err(Stopped);
        }
        // This is an internal shutdown signal only. It deliberately has no
        // representation in the public SearchTermination enum and takes
        // precedence once the principal has ended the team.
        if state & TEAM_DONE != 0 {
            return Err(Stopped);
        }
        if self.cancellation.is_cancelled() {
            self.note_stop(SearchTermination::Cancelled);
            return Err(Stopped);
        }
        // elapsed avoids overflowing Instant for a very large caller Duration.
        if self
            .move_time
            .is_some_and(|limit| self.start.elapsed() >= limit)
        {
            self.note_stop(SearchTermination::TimeLimit);
            return Err(Stopped);
        }
        Ok(())
    }

    fn mark_team_done(&self) {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & TEAM_DONE != 0 {
                return;
            }
            match self.state.compare_exchange_weak(
                state,
                state | TEAM_DONE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(observed) => state = observed,
            }
        }
    }

    fn termination(&self) -> SearchTermination {
        termination_from_code(self.state.load(Ordering::Acquire) & STOP_MASK)
    }

    fn admitted_nodes(&self) -> Option<u64> {
        self.max_nodes
            .map(|_| self.admitted_nodes.load(Ordering::Relaxed))
    }
}

fn stop_code(reason: SearchTermination) -> u8 {
    match reason {
        SearchTermination::Completed => STOP_COMPLETED,
        SearchTermination::NodeLimit => STOP_NODE_LIMIT,
        SearchTermination::TimeLimit => STOP_TIME_LIMIT,
        SearchTermination::Cancelled => STOP_CANCELLED,
    }
}

fn termination_from_code(code: u8) -> SearchTermination {
    match code {
        STOP_NODE_LIMIT => SearchTermination::NodeLimit,
        STOP_TIME_LIMIT => SearchTermination::TimeLimit,
        STOP_CANCELLED => SearchTermination::Cancelled,
        _ => SearchTermination::Completed,
    }
}

/// A worker-local view of the public lifecycle. Each worker counts its own
/// admitted work, while capped searches also use the shared exact admission
/// counter. Recursive code receives only one mutable handle and never clones
/// the cancellation token or reads the clock directly.
pub(crate) struct SearchBudget {
    control: Arc<SharedSearchControl>,
    work_nodes: u64,
}

impl SearchBudget {
    /// Poll the clock/atomic every 256 charges and at public root boundaries.
    /// Node admission is exact; a cap of N admits at most N logical visits.
    const POLL_STRIDE: u64 = 256;

    pub(crate) fn new(limits: SearchLimits, cancellation: CancellationToken) -> Self {
        Self {
            control: SharedSearchControl::new(limits, cancellation),
            work_nodes: 0,
        }
    }

    /// Creates a fresh local counter attached to the same public search.
    pub(crate) fn worker(&self) -> Self {
        Self {
            control: Arc::clone(&self.control),
            work_nodes: 0,
        }
    }

    #[inline]
    pub(crate) fn charge(&mut self) -> Result<(), Stopped> {
        if self.work_nodes & (Self::POLL_STRIDE - 1) == 0 {
            self.control.poll()?;
        }
        if !self.control.try_admit_node() {
            return Err(Stopped);
        }
        self.work_nodes += 1;
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Result<(), Stopped> {
        self.control.poll()
    }

    pub(crate) fn mark_team_done(&self) {
        self.control.mark_team_done();
    }

    pub(crate) fn work_nodes(&self) -> u64 {
        self.work_nodes
    }

    pub(crate) fn termination(&self) -> SearchTermination {
        self.control.termination()
    }

    pub(crate) fn admitted_nodes(&self) -> Option<u64> {
        self.control.admitted_nodes()
    }
}

#[cfg(test)]
impl Default for SearchBudget {
    fn default() -> Self {
        Self::new(SearchLimits::new(0), CancellationToken::new())
    }
}

#[cfg(test)]
mod tests {
    use super::{CancellationToken, SearchBudget, SearchTermination};
    use crate::SearchLimits;

    #[test]
    fn capped_workers_share_exact_admission() {
        let root = SearchBudget::new(
            SearchLimits::new(1).with_max_nodes(10),
            CancellationToken::new(),
        );
        let mut first = root.worker();
        let mut second = root.worker();
        for _ in 0..5 {
            first.charge().unwrap();
            second.charge().unwrap();
        }
        assert_eq!(first.work_nodes() + second.work_nodes(), 10);
        assert!(first.charge().is_err() || second.charge().is_err());
        assert_eq!(root.termination(), SearchTermination::NodeLimit);
        assert_eq!(root.admitted_nodes(), Some(10));
    }

    #[test]
    fn team_done_is_not_a_public_stop_reason() {
        let root = SearchBudget::default();
        let mut helper = root.worker();
        root.mark_team_done();
        assert!(helper.poll().is_err());
        assert_eq!(root.termination(), SearchTermination::Completed);
    }
}
