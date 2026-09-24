//! Tier-aware scheduler.
//!
//! Enforces the **absolute responsiveness rule**: no uncontrolled filesystem or
//! subprocess operation may freeze input.
//!
//! - [`CostTier::MemoryOnly`] / [`CostTier::CheapLocal`] run inline.
//! - [`CostTier::BlockingLocal`] / [`CostTier::Subprocess`] are dispatched to a
//!   bounded worker thread; the editor loop returns immediately with a
//!   [`Pending`](WorkStatus::Pending) status and learns completion through
//!   [`Scheduler::poll`].
//! - [`CostTier::Network`] is rejected on ordinary Tab (M1).

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::budget::{CostTier, DiscoveryBudget};
use crate::outcome::ProviderOutcome;
use crate::provider::ProviderId;

/// Status of background discovery work, mirroring Reedline's
/// `CompletionStatus` seam (`Idle` / `Pending` / `Ready`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStatus {
    /// No background work outstanding.
    Idle,
    /// Background work outstanding.
    Pending,
    /// A background result is ready to collect.
    Ready,
}

/// A queued background job.
struct Job {
    provider: ProviderId,
    tier: CostTier,
    run: Box<dyn FnOnce(&DiscoveryBudget) -> ProviderOutcome + Send>,
    budget: DiscoveryBudget,
}

/// A completed background result.
pub struct WorkResult {
    pub provider: ProviderId,
    pub tier: CostTier,
    pub outcome: ProviderOutcome,
    pub latency: Duration,
}

/// Bounded asynchronous worker for tiers that must not block the editor loop.
///
/// Latest-query-wins: queueing a new job supersedes any queued (not yet
/// started) jobs, so the worker never falls behind the typist.
pub struct Scheduler {
    queue: Arc<Mutex<VecDeque<Job>>>,
    results: Arc<Mutex<VecDeque<WorkResult>>>,
    tx: Sender<()>,
    _rx_keepalive: Receiver<()>,
    handle: Option<thread::JoinHandle<()>>,
    outstanding: Arc<Mutex<usize>>,
}

impl Scheduler {
    /// Spawns the bounded worker thread.
    pub fn spawn() -> Self {
        let queue: Arc<Mutex<VecDeque<Job>>> = Arc::new(Mutex::new(VecDeque::new()));
        let results: Arc<Mutex<VecDeque<WorkResult>>> = Arc::new(Mutex::new(VecDeque::new()));
        let outstanding = Arc::new(Mutex::new(0usize));
        let (tx, rx) = channel::<()>();

        let q = queue.clone();
        let r = results.clone();
        let o = outstanding.clone();

        let handle = thread::Builder::new()
            .name("omen-discovery-worker".into())
            .spawn(move || {
                // Keep the channel alive; it is only a wake-up hint.
                let _keepalive = rx;
                loop {
                    let job = { q.lock().ok().and_then(|mut q| q.pop_front()) };
                    match job {
                        Some(job) => {
                            let started = Instant::now();
                            let outcome = (job.run)(&job.budget);
                            let latency = started.elapsed();
                            if let Ok(mut rs) = r.lock() {
                                if rs.len() > 16 {
                                    rs.pop_front();
                                }
                                rs.push_back(WorkResult {
                                    provider: job.provider,
                                    tier: job.tier,
                                    outcome,
                                    latency,
                                });
                            }
                            if let Ok(mut n) = o.lock() {
                                *n = n.saturating_sub(1);
                            }
                        }
                        None => {
                            // Nothing queued; yield rather than spin.
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                }
            })
            .expect("spawn discovery worker");

        Self {
            queue,
            results,
            tx,
            _rx_keepalive: channel::<()>().1,
            handle: Some(handle),
            outstanding,
        }
    }

    /// Queues a background job, superseding any queued-but-unstarted jobs.
    ///
    /// Returns `false` if the tier must never run (network) or the job cannot
    /// be queued.
    pub fn dispatch(
        &self,
        provider: ProviderId,
        tier: CostTier,
        budget: DiscoveryBudget,
        run: Box<dyn FnOnce(&DiscoveryBudget) -> ProviderOutcome + Send>,
    ) -> bool {
        if tier == CostTier::Network {
            // Tier 3 never runs on ordinary Tab in M1.
            return false;
        }
        if !tier.must_be_async() {
            // Inline tiers should not be dispatched here.
            return false;
        }
        if let Ok(mut q) = self.queue.lock() {
            q.clear(); // latest-query-wins
            q.push_back(Job {
                provider,
                tier,
                run,
                budget,
            });
        }
        if let Ok(mut n) = self.outstanding.lock() {
            *n += 1;
        }
        let _ = self.tx.send(());
        true
    }

    /// Polls background work status (called once per editor-loop iteration).
    pub fn poll(&self) -> WorkStatus {
        let ready = self.results.lock().map(|r| !r.is_empty()).unwrap_or(false);
        if ready {
            return WorkStatus::Ready;
        }
        let pending = self.outstanding.lock().map(|n| *n > 0).unwrap_or(false);
        if pending {
            WorkStatus::Pending
        } else {
            WorkStatus::Idle
        }
    }

    /// Collects the next completed background result, if any.
    pub fn take_result(&self) -> Option<WorkResult> {
        self.results.lock().ok().and_then(|mut r| r.pop_front())
    }

    /// Drains all completed results.
    pub fn drain(&self) -> Vec<WorkResult> {
        self.results
            .lock()
            .map(|mut r| r.drain(..).collect())
            .unwrap_or_default()
    }

    /// Number of jobs queued or running.
    pub fn outstanding(&self) -> usize {
        self.outstanding.lock().map(|n| *n).unwrap_or(0)
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        // The worker thread is detached and exits on process shutdown; we do
        // not join it because the editor may drop the scheduler while the
        // worker is mid-job. Jobs observe their own deadlines/cancellation.
        self.handle.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{DeclineReason, ProviderOutcome};

    #[test]
    fn inline_tiers_are_not_dispatched() {
        let s = Scheduler::spawn();
        assert!(!s.dispatch(
            ProviderId::new("x"),
            CostTier::CheapLocal,
            DiscoveryBudget::inline_only(),
            Box::new(|_| ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch
            }),
        ));
    }

    #[test]
    fn network_tier_is_never_dispatched() {
        let s = Scheduler::spawn();
        assert!(!s.dispatch(
            ProviderId::new("x"),
            CostTier::Network,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(|_| ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch
            }),
        ));
    }

    #[test]
    fn async_work_completes() {
        let s = Scheduler::spawn();
        assert!(s.dispatch(
            ProviderId::new("fs"),
            CostTier::BlockingLocal,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(|_| ProviderOutcome::Declined {
                reason: DeclineReason::NoMatch
            }),
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if s.poll() == WorkStatus::Ready {
                break;
            }
            assert!(Instant::now() < deadline, "background work must complete");
            thread::sleep(Duration::from_millis(2));
        }
        let r = s.take_result().expect("result");
        assert_eq!(r.provider, ProviderId::new("fs"));
    }
}
