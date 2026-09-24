//! Tier-aware scheduler.
//!
//! Enforces the **absolute responsiveness rule**: no uncontrolled filesystem or
//! subprocess operation may freeze input.
//!
//! - [`CostTier::MemoryOnly`] / [`CostTier::CheapLocal`] run inline (the
//!   registry never dispatches them here).
//! - [`CostTier::BlockingLocal`] / [`CostTier::Subprocess`] are dispatched to a
//!   **shared** bounded worker thread; the editor loop returns immediately with
//!   a [`Pending`](WorkStatus::Pending) status and learns completion through
//!   [`Scheduler::poll`].
//! - [`CostTier::Network`] is rejected on ordinary Tab (M1).
//!
//! One process-wide worker serves every [`Scheduler`] instance (session-scoped
//! handles over shared execution). The worker parks on a condition variable
//! when idle, so idle discovery costs nothing — there is no thread farm per
//! Tab, per keystroke or per test.
//!
//! Every work result carries the `operation_id` it was dispatched under, so a
//! late result from a superseded request is recognised as *not this request*
//! instead of being mistaken for current truth.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
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

/// A queued background job. Carries its own execution environment so the
/// shared worker routes results back to the scheduler that owns the work.
struct Job {
    scheduler_id: u64,
    provider: ProviderId,
    tier: CostTier,
    operation_id: u64,
    run: Box<dyn FnOnce(&DiscoveryBudget) -> ProviderOutcome + Send>,
    budget: DiscoveryBudget,
    results: Arc<Mutex<VecDeque<WorkResult>>>,
    outstanding: Arc<AtomicUsize>,
}

/// A completed background result.
pub struct WorkResult {
    pub provider: ProviderId,
    pub tier: CostTier,
    /// The discovery request this work was dispatched under. Results whose
    /// operation no longer matches the live request are discarded, never
    /// presented as current truth.
    pub operation_id: u64,
    pub outcome: ProviderOutcome,
    pub latency: Duration,
}

struct GlobalQueue {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    started: std::sync::atomic::AtomicBool,
}

fn global_queue() -> &'static GlobalQueue {
    static Q: OnceLock<GlobalQueue> = OnceLock::new();
    Q.get_or_init(|| GlobalQueue {
        queue: Mutex::new(VecDeque::new()),
        wake: Condvar::new(),
        started: std::sync::atomic::AtomicBool::new(false),
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the shared worker once per process.
fn ensure_worker() {
    let gq = global_queue();
    if gq.started.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("omen-discovery-worker".into())
        .spawn(worker_loop)
        .expect("spawn discovery worker");
}

fn worker_loop() {
    let gq = global_queue();
    let mut q = lock(&gq.queue);
    loop {
        while q.is_empty() {
            // Park until work arrives. The timeout is only a liveness safety
            // net; jobs carry their own deadlines.
            q = gq
                .wake
                .wait_timeout(q, Duration::from_secs(30))
                .map(|(guard, _)| guard)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        let job = q.pop_front().expect("queue non-empty under lock");
        drop(q);

        let started = Instant::now();
        let outcome = (job.run)(&job.budget);
        let latency = started.elapsed();

        if let Ok(mut rs) = job.results.lock() {
            if rs.len() > 16 {
                rs.pop_front();
            }
            rs.push_back(WorkResult {
                provider: job.provider,
                tier: job.tier,
                operation_id: job.operation_id,
                outcome,
                latency,
            });
        }
        job.outstanding.fetch_sub(1, Ordering::SeqCst);

        q = lock(&gq.queue);
    }
}

/// Session-scoped handle onto the shared asynchronous worker.
///
/// Cloning is cheap; every handle sees only its own results, so concurrent
/// runtimes (live session, tests, static tooling) never steal each other's
/// truth.
#[derive(Clone)]
pub struct Scheduler {
    id: u64,
    results: Arc<Mutex<VecDeque<WorkResult>>>,
    outstanding: Arc<AtomicUsize>,
}

impl Scheduler {
    /// Attaches to the shared worker (started lazily on first dispatch).
    pub fn spawn() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
            results: Arc::new(Mutex::new(VecDeque::new())),
            outstanding: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Queues a background job, superseding any of **this scheduler's**
    /// queued-but-unstarted jobs (latest-query-wins, scoped to this session).
    ///
    /// The job runs the work it carries — the provider identity, cost tier and
    /// output stay bound together. Returns `false` if the tier must never run
    /// (network) or must not be dispatched (inline tiers).
    pub fn dispatch(
        &self,
        provider: ProviderId,
        tier: CostTier,
        operation_id: u64,
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

        ensure_worker();
        let gq = global_queue();
        {
            let mut q = lock(&gq.queue);
            let mut superseded = 0usize;
            q.retain(|j| {
                if j.scheduler_id == self.id {
                    superseded += 1;
                    false
                } else {
                    true
                }
            });
            if superseded > 0 {
                // Superseded jobs never run, so refund their outstanding
                // count; otherwise poll() would report Pending forever.
                self.outstanding.fetch_sub(
                    superseded.min(self.outstanding.load(Ordering::SeqCst)),
                    Ordering::SeqCst,
                );
            }
            q.push_back(Job {
                scheduler_id: self.id,
                provider,
                tier,
                operation_id,
                run,
                budget,
                results: self.results.clone(),
                outstanding: self.outstanding.clone(),
            });
        }
        self.outstanding.fetch_add(1, Ordering::SeqCst);
        gq.wake.notify_one();
        true
    }

    /// Polls background work status (called once per editor-loop iteration).
    pub fn poll(&self) -> WorkStatus {
        let ready = self.results.lock().map(|r| !r.is_empty()).unwrap_or(false);
        if ready {
            return WorkStatus::Ready;
        }
        let pending = self.outstanding.load(Ordering::SeqCst) > 0;
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

    /// Number of jobs queued or running for this scheduler.
    pub fn outstanding(&self) -> usize {
        self.outstanding.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{DeclineReason, ProviderOutcome};
    use std::sync::Condvar as StdCondvar;

    fn declined() -> ProviderOutcome {
        ProviderOutcome::Declined {
            reason: DeclineReason::NoMatch,
        }
    }

    fn await_ready(s: &Scheduler) -> WorkResult {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(r) = s.take_result() {
                return r;
            }
            assert!(Instant::now() < deadline, "background work must complete");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn await_idle(s: &Scheduler) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if s.outstanding() == 0 && s.poll() == WorkStatus::Idle {
                return;
            }
            assert!(Instant::now() < deadline, "scheduler must drain to idle");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn inline_tiers_are_not_dispatched() {
        let s = Scheduler::spawn();
        assert!(!s.dispatch(
            ProviderId::new("x"),
            CostTier::CheapLocal,
            0,
            DiscoveryBudget::inline_only(),
            Box::new(|_| declined()),
        ));
    }

    #[test]
    fn network_tier_is_never_dispatched() {
        let s = Scheduler::spawn();
        assert!(!s.dispatch(
            ProviderId::new("x"),
            CostTier::Network,
            0,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(|_| declined()),
        ));
    }

    #[test]
    fn async_work_completes() {
        let s = Scheduler::spawn();
        assert!(s.dispatch(
            ProviderId::new("fs"),
            CostTier::BlockingLocal,
            7,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(|_| declined()),
        ));
        let r = await_ready(&s);
        assert_eq!(r.provider, ProviderId::new("fs"));
        assert_eq!(r.tier, CostTier::BlockingLocal, "real tier survives");
        assert_eq!(r.operation_id, 7, "operation identity survives");
        await_idle(&s);
    }

    #[test]
    fn superseded_jobs_do_not_leak_pending_state() {
        // Regression: latest-query-wins used to drop queued jobs without
        // refunding their outstanding count, leaving poll() Pending forever.
        let s = Scheduler::spawn();
        for i in 0..5 {
            assert!(s.dispatch(
                ProviderId::new("fs"),
                CostTier::BlockingLocal,
                i,
                DiscoveryBudget::allowing_subprocess(),
                Box::new(|_| declined()),
            ));
        }
        let r = await_ready(&s);
        assert_eq!(r.operation_id, 4, "only the latest job runs");
        await_idle(&s);
        assert_eq!(s.poll(), WorkStatus::Idle, "no phantom Pending");
    }

    #[test]
    fn schedulers_are_isolated_even_when_superseding() {
        // The shared worker serves every scheduler; superseding one session's
        // queue must never drop another session's work.
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release = Arc::new((Mutex::new(false), StdCondvar::new()));
        let busy = Scheduler::spawn();
        let other = Scheduler::spawn();

        let s_flag = started.clone();
        let g = release.clone();
        assert!(busy.dispatch(
            ProviderId::new("gate"),
            CostTier::Subprocess,
            100,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(move |_| {
                s_flag.store(true, Ordering::SeqCst);
                let (m, cv) = &*g;
                let mut open = lock(m);
                let deadline = Instant::now() + Duration::from_secs(5);
                while !*open && Instant::now() < deadline {
                    open = cv
                        .wait_timeout(open, Duration::from_millis(50))
                        .map(|(guard, _)| guard)
                        .unwrap_or_else(|e| e.into_inner().0);
                }
                declined()
            }),
        ));
        // Deterministic hook: wait until the worker is actually executing the
        // gate job (so the queue manipulations below race nothing).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "gate job must start");
            std::thread::sleep(Duration::from_millis(2));
        }

        // This session supersedes its own queued work twice while the worker
        // is occupied...
        for i in 0..2 {
            assert!(busy.dispatch(
                ProviderId::new("busy-late"),
                CostTier::BlockingLocal,
                101 + i,
                DiscoveryBudget::allowing_subprocess(),
                Box::new(|_| declined()),
            ));
        }
        // ...while another session dispatches independently.
        assert!(other.dispatch(
            ProviderId::new("other"),
            CostTier::BlockingLocal,
            200,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(|_| declined()),
        ));

        // Release the gate.
        {
            let (m, cv) = &*release;
            *lock(m) = true;
            cv.notify_all();
        }

        // Busy session: the gate result, then exactly its LATEST queued job
        // (operation 102); the superseded 101 must never appear.
        let r1 = await_ready(&busy);
        assert_eq!(r1.provider, ProviderId::new("gate"));
        let r2 = await_ready(&busy);
        assert_eq!(r2.provider, ProviderId::new("busy-late"));
        assert_eq!(r2.operation_id, 102, "latest query wins");
        // A third result here would mean the superseded job leaked through.
        await_idle(&busy);

        // Other session: its own result arrived untouched by busy's clears.
        let r3 = await_ready(&other);
        assert_eq!(r3.provider, ProviderId::new("other"));
        assert_eq!(r3.operation_id, 200);
        await_idle(&other);
    }

    #[test]
    fn poll_reports_ready_then_returns_to_idle() {
        let s = Scheduler::spawn();
        assert!(s.poll() == WorkStatus::Idle);
        assert!(s.dispatch(
            ProviderId::new("p"),
            CostTier::Subprocess,
            1,
            DiscoveryBudget::allowing_subprocess(),
            Box::new(|_| declined()),
        ));
        let _ = await_ready(&s);
        await_idle(&s);
    }
}
