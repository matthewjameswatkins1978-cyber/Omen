//! Cost tiers, budgets and cancellation.
//!
//! **Absolute responsiveness rule:** no uncontrolled filesystem or subprocess
//! operation may freeze input. Local does not mean cheap. If cost is uncertain,
//! treat it as asynchronous. The input/editor loop retains control.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Cost tier of a discovery operation.
///
/// `Local` does not imply `Inline`: see [`CostTier::may_run_inline`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CostTier {
    /// Guaranteed in-memory / immediate. May run inline.
    MemoryOnly,
    /// Known-cheap bounded local state already indexed/cached/in-memory.
    /// May run inline under a strict bounded path.
    CheapLocal,
    /// Potentially blocking local I/O (`readdir`, `stat` on unknown storage,
    /// removable drive, network-backed path, pathological filesystem).
    /// **Must not** block the editor loop; dispatched through a bounded
    /// asynchronous worker/cache path.
    BlockingLocal,
    /// Bounded subprocess discovery/harvest. Async only, cached, deadline,
    /// cancellation, closed stdin, bounded output.
    Subprocess,
    /// Network / external. Declared for architecture only. Must not run on
    /// ordinary Tab in M1.
    Network,
}

impl CostTier {
    /// Whether this tier is permitted to run synchronously on the editor thread.
    ///
    /// Only memory-only and known-cheap bounded local state may run inline.
    pub fn may_run_inline(self) -> bool {
        matches!(self, CostTier::MemoryOnly | CostTier::CheapLocal)
    }

    /// Whether this tier must be dispatched through the async worker path.
    pub fn must_be_async(self) -> bool {
        !self.may_run_inline()
    }

    pub fn label(self) -> &'static str {
        match self {
            CostTier::MemoryOnly => "TIER_0",
            CostTier::CheapLocal => "TIER_1A",
            CostTier::BlockingLocal => "TIER_1B",
            CostTier::Subprocess => "TIER_2",
            CostTier::Network => "TIER_3",
        }
    }
}

/// How deep discovery may look for this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DiscoveryDepth {
    /// Shallow: memory-only and cached results.
    Shallow,
    /// Normal: bounded local work permitted.
    Normal,
    /// Deep: broader bounded work permitted (still tier-gated).
    Deep,
}

/// Cooperative cancellation token shared between the editor loop and workers.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }
}

/// Bounded resources for one discovery request.
///
/// Every external boundary must have a deadline or cancellation path
/// (standing rule 1). A provider that ignores the budget is a bug.
#[derive(Debug, Clone)]
pub struct DiscoveryBudget {
    /// Highest cost tier permitted for this request.
    pub tier_ceiling: CostTier,
    /// Hard wall-clock bound for the whole request.
    pub deadline: Duration,
    /// Cooperative cancellation.
    pub cancel: CancellationToken,
    /// Per-provider candidate cap (context economy).
    pub max_candidates: usize,
    /// Bounded output bytes.
    pub max_bytes: usize,
    /// When the request started.
    pub started_at: Instant,
}

impl DiscoveryBudget {
    /// A budget that permits only inline tiers.
    pub fn inline_only() -> Self {
        Self {
            tier_ceiling: CostTier::CheapLocal,
            deadline: Duration::from_millis(50),
            cancel: CancellationToken::new(),
            max_candidates: 256,
            max_bytes: 64 * 1024,
            started_at: Instant::now(),
        }
    }

    /// A budget that permits cached subprocess harvest.
    pub fn allowing_subprocess() -> Self {
        Self {
            tier_ceiling: CostTier::Subprocess,
            deadline: Duration::from_millis(1500),
            cancel: CancellationToken::new(),
            max_candidates: 512,
            max_bytes: 256 * 1024,
            started_at: Instant::now(),
        }
    }

    /// Whether `tier` is permitted under this budget.
    pub fn permits(&self, tier: CostTier) -> bool {
        tier <= self.tier_ceiling
    }

    /// Whether the deadline has elapsed or cancellation was requested.
    pub fn is_exhausted(&self) -> bool {
        self.cancel.is_cancelled() || self.started_at.elapsed() >= self.deadline
    }

    /// Remaining time, saturating at zero.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_sub(self.started_at.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_memory_and_cheap_local_may_run_inline() {
        assert!(CostTier::MemoryOnly.may_run_inline());
        assert!(CostTier::CheapLocal.may_run_inline());
        assert!(!CostTier::BlockingLocal.may_run_inline());
        assert!(!CostTier::Subprocess.may_run_inline());
        assert!(!CostTier::Network.may_run_inline());
    }

    #[test]
    fn blocking_local_and_above_must_be_async() {
        assert!(CostTier::BlockingLocal.must_be_async());
        assert!(CostTier::Subprocess.must_be_async());
        assert!(CostTier::Network.must_be_async());
        assert!(!CostTier::MemoryOnly.must_be_async());
    }

    #[test]
    fn inline_budget_forbids_subprocess() {
        let b = DiscoveryBudget::inline_only();
        assert!(b.permits(CostTier::CheapLocal));
        assert!(!b.permits(CostTier::Subprocess));
        assert!(!b.permits(CostTier::Network));
    }

    #[test]
    fn subprocess_budget_still_forbids_network() {
        let b = DiscoveryBudget::allowing_subprocess();
        assert!(b.permits(CostTier::Subprocess));
        assert!(!b.permits(CostTier::Network));
    }

    #[test]
    fn cancellation_is_observed() {
        let b = DiscoveryBudget::inline_only();
        assert!(!b.is_exhausted());
        b.cancel.cancel();
        assert!(b.is_exhausted());
    }

    #[test]
    fn deadline_is_observed() {
        let mut b = DiscoveryBudget::inline_only();
        b.deadline = Duration::ZERO;
        assert!(b.is_exhausted());
    }
}
