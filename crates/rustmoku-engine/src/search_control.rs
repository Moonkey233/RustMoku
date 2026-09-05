//! Per-public-search lifecycle. Logical work is distinct from subsystem stats.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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

pub(crate) struct SearchBudget {
    work_nodes: u64,
    max_nodes: u64,
    start: Instant,
    move_time: Option<Duration>,
    cancellation: CancellationToken,
    stop_reason: SearchTermination,
}

impl SearchBudget {
    /// Poll the clock/atomic every 256 charges and at public root boundaries.
    /// Node admission is exact; a cap of N admits at most N logical visits.
    const POLL_STRIDE: u64 = 256;

    pub(crate) fn new(limits: SearchLimits, cancellation: CancellationToken) -> Self {
        Self {
            work_nodes: 0,
            max_nodes: limits.max_nodes.unwrap_or(u64::MAX),
            start: Instant::now(),
            move_time: limits.move_time,
            cancellation,
            stop_reason: SearchTermination::Completed,
        }
    }

    #[inline]
    pub(crate) fn charge(&mut self) -> Result<(), Stopped> {
        if self.work_nodes >= self.max_nodes {
            return self.stop(SearchTermination::NodeLimit);
        }
        if self.work_nodes & (Self::POLL_STRIDE - 1) == 0 {
            self.poll()?;
        }
        self.work_nodes += 1;
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Result<(), Stopped> {
        if self.stop_reason != SearchTermination::Completed {
            return Err(Stopped);
        }
        if self.cancellation.is_cancelled() {
            return self.stop(SearchTermination::Cancelled);
        }
        // elapsed avoids overflowing Instant for a very large caller Duration.
        if self
            .move_time
            .is_some_and(|limit| self.start.elapsed() >= limit)
        {
            return self.stop(SearchTermination::TimeLimit);
        }
        Ok(())
    }

    fn stop(&mut self, reason: SearchTermination) -> Result<(), Stopped> {
        if self.stop_reason == SearchTermination::Completed {
            self.stop_reason = reason;
        }
        Err(Stopped)
    }

    pub(crate) fn work_nodes(&self) -> u64 {
        self.work_nodes
    }
    pub(crate) fn termination(&self) -> SearchTermination {
        self.stop_reason
    }
}

#[cfg(test)]
impl Default for SearchBudget {
    fn default() -> Self {
        Self::new(SearchLimits::new(0), CancellationToken::new())
    }
}
