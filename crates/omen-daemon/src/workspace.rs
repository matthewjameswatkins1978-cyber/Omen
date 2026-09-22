use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast, watch};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use omen_core::{CoreError, ExecutionId, InteractiveSessionId};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_ipc::{
    CancelOutcome, CancelRecord, EventPayload, ExecutionResultSummary, FactInfo, IpcEvent,
    LocalIpcError, ManagedServiceInfo, SharedIndexSnapshot,
};
use omen_knowledge::{
    Database, ExecutionHistory, ExecutionRecord, FactRegistry, HistoryStatusEnvelope,
    RequestReceiptRecord, ServiceRecord, WorkspacePersistence, canonical_workspace_db_path,
    cas::ContentAddressedStore, deterministic_workspace_id, resolve_workspace_dir,
};

pub type InFlightMap =
    Arc<Mutex<HashMap<String, broadcast::Sender<Result<ExecutionResultSummary, LocalIpcError>>>>>;

/// Bounded wait for the terminal broadcast after firing a cancellation flag.
/// Expiry reports `OutcomeUnknown`; the caller never hangs on a wedged task.
pub const CANCEL_OBSERVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Reproduce a recorded terminal runtime for the dedup-replay path.
///
/// The broker stamps every history record with an explicit status envelope.
/// `FAILED` (and the physical failure spellings) replay as `Completed`
/// because the original summary reported physical completion with a
/// non-zero exit; the exit code itself carries the failure. Returns `None`
/// for records without a parsable envelope (legacy rows).
/// Repair A: resolve the exact physical runtime for durable replay.
///
/// Exact typed truth wins: broker-stamped envelopes carry `runtime_status`
/// verbatim and round-trip every `RuntimeStatus` variant, so the same
/// physical execution can never change its story across a restart
/// (`IoFailed -> FAILED -> Completed` is the canonical forbidden
/// rewrite). The coarse `status` label is presentation only and is NEVER
/// used to reconstruct precision.
///
/// Legacy rows without exact information fall back to narrow safe
/// mappings; genuinely ambiguous rows return `None` and the caller
/// refuses replay. Missing exact historical information is not permission
/// to invent it — refusal is safe because the identity stays consumed and
/// must not physically execute again.
fn replay_runtime_status(
    rec: &ExecutionRecord,
    receipt_status: &str,
) -> Option<omen_core::RuntimeStatus> {
    use omen_core::RuntimeStatus as Exact;
    if let Some(envelope) = rec
        .envelope_json
        .as_deref()
        .and_then(HistoryStatusEnvelope::parse)
    {
        if let Some(exact) = envelope.runtime_status {
            return Some(exact);
        }
        // Legacy envelope: coarse label only. 1:1 labels replay exactly.
        // FAILED is ambiguous (Completed-with-nonzero-exit vs IoFailed vs
        // spawn/containment failures) — only an actual child exit code
        // proves a Completed origin, because that is the sole
        // FAILED-with-exit producer. Anything else refuses.
        return match envelope.status.as_str() {
            "COMPLETED" => Some(Exact::Completed),
            "TIMED_OUT" => Some(Exact::TimedOut),
            "CANCELLED" => Some(Exact::Cancelled),
            "OUTCOME_UNKNOWN" => Some(Exact::OutcomeUnknown),
            "FAILED" if rec.exit_code.is_some() => Some(Exact::Completed),
            _ => None,
        };
    }
    // Envelope-less rows predate envelope stamping: keep the narrow
    // pre-existing fallback (a Cancelled receipt without an envelope
    // replays Cancelled; exit present → Completed, absent → TimedOut).
    if receipt_status == "Cancelled" {
        Some(Exact::Cancelled)
    } else {
        match rec.exit_code {
            Some(_) => Some(Exact::Completed),
            None => Some(Exact::TimedOut),
        }
    }
}

/// Repair 2: recover the typed dispatch-prevention flag from a history
/// envelope. Rows recorded before the flag existed report false.
fn recorded_dispatch_prevented(rec: &ExecutionRecord) -> bool {
    rec.envelope_json
        .as_deref()
        .and_then(HistoryStatusEnvelope::parse)
        .map(|envelope| envelope.dispatch_prevented)
        .unwrap_or(false)
}

/// Repair 3: deterministic persistence-failure injection for the repair
/// proofs. Production code always runs `Off`. A narrow failpoint is used
/// instead of filesystem sabotage so the faults are exact and bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PersistenceFailpoint {
    /// Normal operation: every persistence write is attempted for real.
    #[default]
    Off,
    /// Refuse the pre-dispatch `Running` receipt write (must block spawn).
    FailRunningReceipt,
    /// Refuse terminal history writes after physical execution.
    FailHistoryWrite,
    /// Refuse terminal receipt writes after history succeeded.
    FailTerminalReceipt,
    /// Fail every durable request-receipt (`get_request_receipt`) read.
    /// Repair Preview 12: lets tests prove a receipt-read failure can never
    /// authorize dispatch or overwrite a consumed receipt.
    FailRequestReceiptRead,
    /// Fail every durable execution-ID (`get_receipt_by_execution_id`) read.
    /// Repair Preview 12: lets tests prove a cancel lookup failure can never
    /// become `NotFound`.
    FailReceiptByExecRead,
    /// Let exactly `u64` durable receipt reads succeed, then fail every
    /// subsequent read until disarmed. Repair Preview 12: makes the cancel
    /// durable-reread path (initial lookup OK, reread fails)
    /// deterministically reachable — it is otherwise unobservable from
    /// outside a single `cancel_execution` call.
    FailReceiptReadAfterSuccesses(u64),
}

/// Which receipt write is being attempted. Injection targets the broker's
/// pre-dispatch (`Running`) and terminal writes only; cancel-path intent
/// and reconcile annotations (`Other`) always attempt the real write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiptStage {
    Running,
    Terminal,
    Other,
}

/// Repair 2 test seam: parks the broker task after it registers its live
/// cancellation switch and BEFORE it calls into the execution backend, so
/// repair tests can deterministically fire a cancel pre-dispatch, then
/// release the task and observe `DispatchPrevented` with zero spawns.
#[derive(Debug, Clone, Default)]
pub struct PrespawnGate {
    /// Signalled by the broker task once it is parked pre-dispatch.
    pub task_parked: Arc<tokio::sync::Notify>,
    /// Signalled by the test to release the parked broker task.
    pub release: Arc<tokio::sync::Notify>,
}

pub struct ManagedChildService {
    pub name: String,
    pub command: String,
    pub argv: Vec<String>,
    pub pid: u32,
    pub lease_id: omen_core::RuntimeLeaseId,
    pub started_at: std::time::Instant,
    pub log_lines: Arc<RwLock<Vec<String>>>,
    pub stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

pub type ManagedProcessMap = Arc<Mutex<HashMap<String, ManagedChildService>>>;

#[derive(Debug, Clone)]
pub struct BrokerExecutionParams<'a> {
    pub dedup_id: &'a str,
    pub session_id: &'a str,
    pub tool: &'a str,
    pub operation: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub timeout_ms: u64,
}

pub struct WorkspaceState {
    workspace_id: String,
    canonical_path: PathBuf,
    epoch: u64,
    sequence: Arc<AtomicU64>,
    facts: RwLock<HashMap<String, FactInfo>>,
    services: RwLock<HashMap<String, ManagedServiceInfo>>,
    service_configs: RwLock<HashMap<String, (String, Vec<String>)>>,
    managed_processes: ManagedProcessMap,
    db: Arc<Mutex<Database>>,
    cas: Arc<ContentAddressedStore>,
    supervisor: Arc<ProcessSupervisor>,
    execution_cache: RwLock<HashMap<String, ExecutionResultSummary>>,
    in_flight_executions: InFlightMap,
    /// E2 stop truth: live cancellation flags keyed by canonical execution
    /// ID. Firing a flag records INTENT; the broker task still performs the
    /// physical stop and observes the outcome (confirm or unknown).
    cancel_switches: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// Execution ID → dedup (consequential request) ID for in-flight work.
    /// Needed to observe the terminal broadcast after firing a cancel flag.
    cancel_index: Mutex<HashMap<String, String>>,
    /// Repair 3: persistence-failure injection (test seam; `Off` in
    /// production). Guarded by a std mutex and never held across await.
    persistence_failpoint: std::sync::Mutex<PersistenceFailpoint>,
    /// Repair 2: pre-dispatch parking gate (test seam; `None` in
    /// production). Guarded by a std mutex and never held across await.
    prespawn_gate: std::sync::Mutex<Option<PrespawnGate>>,
    event_tx: broadcast::Sender<IpcEvent>,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
}

impl WorkspaceState {
    pub fn new(canonical_path: PathBuf, epoch: u64) -> Result<Self, LocalIpcError> {
        Self::new_with_supervisor(canonical_path, epoch, Arc::new(ProcessSupervisor::new()))
    }

    /// Repair A test seam: construct with an injected execution supervisor
    /// (e.g. a deterministic stub backend returning `IoFailed` through the
    /// real supervisor/broker path). Production always uses `new()`.
    #[doc(hidden)]
    pub fn new_with_supervisor(
        canonical_path: PathBuf,
        epoch: u64,
        supervisor: Arc<ProcessSupervisor>,
    ) -> Result<Self, LocalIpcError> {
        let workspace_id = deterministic_workspace_id(&canonical_path);
        let state_dir = resolve_workspace_dir(&canonical_path);
        let db_path = canonical_workspace_db_path(&canonical_path);
        let db = Database::open(&db_path).map_err(|e| {
            LocalIpcError::DaemonDegraded(format!(
                "Failed to open canonical database at {}: {e}",
                db_path.display()
            ))
        })?;

        let _ = WorkspacePersistence::upsert_workspace(
            &db,
            &workspace_id,
            &canonical_path.to_string_lossy(),
            epoch,
        );

        // Crash reconciliation: Running request receipts become Unknown
        let _ = WorkspacePersistence::reconcile_running_receipts(&db);

        let mut initial_facts = HashMap::new();
        if let Ok(records) = FactRegistry::list_all_facts(&db) {
            for r in records {
                let resource_uri = r.resource_uri.to_string();
                let fact_id = r.fact_id.to_string();
                let validity = match r.validity {
                    omen_core::ValidityState::Current => "CURRENT",
                    omen_core::ValidityState::Dirty => "DIRTY",
                    omen_core::ValidityState::Stale => "STALE",
                    omen_core::ValidityState::Superseded => "SUPERSEDED",
                    omen_core::ValidityState::Historical => "HISTORICAL",
                }
                .to_string();
                let assurance = match r.assurance {
                    omen_core::Assurance::Deterministic => "DETERMINISTIC",
                    omen_core::Assurance::Verified => "VERIFIED",
                    omen_core::Assurance::Enforced => "ENFORCED",
                    omen_core::Assurance::Observed => "OBSERVED",
                    omen_core::Assurance::Claimed => "CLAIMED",
                    omen_core::Assurance::Inferred => "INFERRED",
                    omen_core::Assurance::Unknown => "UNKNOWN",
                }
                .to_string();
                let value = r.value;
                initial_facts.insert(
                    resource_uri.clone(),
                    FactInfo {
                        fact_id,
                        resource_uri,
                        value,
                        validity,
                        assurance,
                    },
                );
            }
        }

        let mut initial_services = HashMap::new();
        if let Ok(records) = WorkspacePersistence::list_services(&db, &workspace_id) {
            for s in records {
                let mut state = s.state.clone();
                let mut pid = s.pid;
                // Crash reconciliation: if recorded as running or observed
                if state == "running" || state == "observed" {
                    let is_alive = pid.map(omen_engine::is_process_alive).unwrap_or(false);
                    if is_alive {
                        // Alive in OS, but not managed/owned by this new daemon process
                        state = "observed".to_string();
                        let _ = WorkspacePersistence::upsert_service(
                            &db,
                            &ServiceRecord {
                                workspace_id: workspace_id.clone(),
                                name: s.name.clone(),
                                command: s.command.clone(),
                                pid,
                                state: state.clone(),
                                started_at: s.started_at.clone(),
                                updated_at: chrono::Utc::now().to_rfc3339(),
                            },
                        );
                    } else {
                        state = "crashed".to_string();
                        pid = None;
                        let _ = WorkspacePersistence::upsert_service(
                            &db,
                            &ServiceRecord {
                                workspace_id: workspace_id.clone(),
                                name: s.name.clone(),
                                command: s.command.clone(),
                                pid: None,
                                state: state.clone(),
                                started_at: s.started_at.clone(),
                                updated_at: chrono::Utc::now().to_rfc3339(),
                            },
                        );
                    }
                }
                initial_services.insert(
                    s.name.clone(),
                    ManagedServiceInfo {
                        name: s.name.clone(),
                        resource_uri: format!("proc://workspace/{}", s.name),
                        pid,
                        command: s.command,
                        state,
                        uptime_secs: 0,
                        lease_id: None,
                    },
                );
            }
        }

        let (event_tx, _) = broadcast::channel(512);
        let cas = Arc::new(ContentAddressedStore::new(state_dir.join("cas")));
        let execution_cache = RwLock::new(HashMap::new());
        let in_flight_executions = Arc::new(Mutex::new(HashMap::new()));
        let managed_processes = Arc::new(Mutex::new(HashMap::new()));
        let service_configs = RwLock::new(HashMap::new());

        Ok(Self {
            workspace_id,
            canonical_path,
            epoch,
            sequence: Arc::new(AtomicU64::new(1)),
            facts: RwLock::new(initial_facts),
            services: RwLock::new(initial_services),
            service_configs,
            managed_processes,
            db: Arc::new(Mutex::new(db)),
            cas,
            supervisor,
            execution_cache,
            in_flight_executions,
            cancel_switches: Mutex::new(HashMap::new()),
            cancel_index: Mutex::new(HashMap::new()),
            persistence_failpoint: std::sync::Mutex::new(PersistenceFailpoint::Off),
            prespawn_gate: std::sync::Mutex::new(None),
            event_tx,
            watcher: Arc::new(Mutex::new(None)),
        })
    }

    /// Repair 3 test seam: arm deterministic persistence-failure injection.
    /// Production paths never call this; it stays `Off` outside repair tests.
    #[doc(hidden)]
    pub fn set_persistence_failpoint(&self, failpoint: PersistenceFailpoint) {
        if let Ok(mut slot) = self.persistence_failpoint.lock() {
            *slot = failpoint;
        }
    }

    fn persistence_failpoint(&self) -> PersistenceFailpoint {
        self.persistence_failpoint
            .lock()
            .map(|slot| *slot)
            .unwrap_or(PersistenceFailpoint::Off)
    }

    /// Repair 2 test seam: park the next broker task pre-dispatch.
    /// Production paths never call this; it stays `None` outside repair tests.
    #[doc(hidden)]
    pub fn set_prespawn_gate(&self, gate: Option<PrespawnGate>) {
        if let Ok(mut slot) = self.prespawn_gate.lock() {
            *slot = gate;
        }
    }

    fn prespawn_gate_snapshot(&self) -> Option<PrespawnGate> {
        self.prespawn_gate.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn is_ignored_path(path: &Path) -> bool {
        for component in path.components() {
            if let std::path::Component::Normal(c) = component {
                let s = c.to_string_lossy();
                if s == ".git"
                    || s == "target"
                    || s == "node_modules"
                    || s == ".omen"
                    || s == ".omen-state"
                {
                    return true;
                }
            }
        }
        false
    }

    pub async fn invalidate_all_current_facts(&self, cause: &str) -> Vec<String> {
        let mut invalidated = Vec::new();
        let mut facts = self.facts.write().await;
        for fact in facts.values_mut() {
            if fact.validity == "CURRENT" {
                fact.validity = "DIRTY".to_string();
                invalidated.push((fact.fact_id.clone(), fact.resource_uri.clone()));
            }
        }
        drop(facts);

        for (fact_id, resource_uri) in &invalidated {
            self.broadcast_event(EventPayload::FactInvalidated {
                fact_id: fact_id.clone(),
                resource_uri: resource_uri.clone(),
                previous_validity: "CURRENT".into(),
                new_validity: "DIRTY".into(),
                cause: cause.to_string(),
            });
        }

        invalidated.into_iter().map(|(_, uri)| uri).collect()
    }

    pub fn start_fs_watcher(self: &Arc<Self>) -> notify::Result<()> {
        let this = Arc::downgrade(self);
        let rt_handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return Ok(()),
        };
        let rt_clone = rt_handle.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let has_relevant_change = event.paths.iter().any(|p| !Self::is_ignored_path(p));
                    if has_relevant_change && let Some(ws) = this.upgrade() {
                        let cause = format!("fs:mutation:{:?}", event.kind);
                        rt_clone.spawn(async move {
                            ws.invalidate_all_current_facts(&cause).await;
                        });
                    }
                }
            },
            Config::default(),
        )?;

        watcher.watch(&self.canonical_path, RecursiveMode::Recursive)?;

        let watcher_mutex = self.watcher.clone();
        rt_handle.spawn(async move {
            let mut w = watcher_mutex.lock().await;
            *w = Some(watcher);
        });

        Ok(())
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn current_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<IpcEvent> {
        self.event_tx.subscribe()
    }

    pub fn broadcast_event(&self, payload: EventPayload) -> IpcEvent {
        let seq = self.next_sequence();
        let event = IpcEvent::new(&self.workspace_id, self.epoch, seq, payload);
        let _ = self.event_tx.send(event.clone());
        event
    }

    pub fn broadcast_raw_event(&self, event: IpcEvent) -> IpcEvent {
        let _ = self.event_tx.send(event.clone());
        event
    }

    pub async fn get_snapshot(&self) -> SharedIndexSnapshot {
        let facts = self.facts.read().await;
        let services = self.services.read().await;

        let facts_vec: Vec<FactInfo> = facts.values().cloned().collect();
        let services_vec: Vec<ManagedServiceInfo> = services.values().cloned().collect();
        let dirty_count = facts_vec.iter().filter(|f| f.validity == "DIRTY").count();

        SharedIndexSnapshot {
            workspace_id: self.workspace_id.clone(),
            epoch: self.epoch,
            sequence: self.current_sequence(),
            facts: facts_vec,
            services: services_vec,
            tools: vec!["exec".into(), "fs".into(), "git".into(), "cargo".into()],
            dirty_facts_count: dirty_count,
        }
    }

    pub async fn query_fact(&self, resource_uri: &str) -> Option<FactInfo> {
        let facts = self.facts.read().await;
        facts.get(resource_uri).cloned()
    }

    pub async fn put_fact(&self, fact: FactInfo) {
        let resource_uri = fact.resource_uri.clone();
        let fact_id = fact.fact_id.clone();
        let validity = fact.validity.clone();
        let assurance = fact.assurance.clone();

        {
            let mut facts = self.facts.write().await;
            facts.insert(resource_uri.clone(), fact);
        }

        self.broadcast_event(EventPayload::FactPublished {
            fact_id,
            resource_uri,
            validity,
            assurance,
        });
    }

    pub async fn invalidate_fact(&self, resource_uri: &str, cause: &str) -> bool {
        let mut facts = self.facts.write().await;
        if let Some(fact) = facts.get_mut(resource_uri) {
            let previous = fact.validity.clone();
            fact.validity = "DIRTY".to_string();
            let fact_id = fact.fact_id.clone();
            let res_uri = fact.resource_uri.clone();

            drop(facts);

            self.broadcast_event(EventPayload::FactInvalidated {
                fact_id,
                resource_uri: res_uri,
                previous_validity: previous,
                new_validity: "DIRTY".into(),
                cause: cause.to_string(),
            });
            true
        } else {
            false
        }
    }

    pub async fn list_services(&self) -> Vec<ManagedServiceInfo> {
        let services = self.services.read().await;
        let managed = self.managed_processes.lock().await;
        let mut list = Vec::new();
        for svc in services.values() {
            let mut info = svc.clone();
            if let Some(child) = managed.get(&info.name) {
                info.uptime_secs = child.started_at.elapsed().as_secs();
            }
            list.push(info);
        }
        list
    }

    pub async fn get_service(&self, name: &str) -> Option<ManagedServiceInfo> {
        let services = self.services.read().await;
        if let Some(svc) = services.get(name) {
            let mut info = svc.clone();
            let managed = self.managed_processes.lock().await;
            if let Some(child) = managed.get(name) {
                info.uptime_secs = child.started_at.elapsed().as_secs();
            }
            Some(info)
        } else {
            None
        }
    }

    pub async fn start_managed_service(
        self: &Arc<Self>,
        name: &str,
        command: &str,
        argv: &[String],
    ) -> Result<ManagedServiceInfo, LocalIpcError> {
        {
            let services = self.services.read().await;
            if let Some(existing) = services.get(name)
                && existing.state == "running"
            {
                return Err(LocalIpcError::ServiceAlreadyRunning(format!(
                    "Service '{name}' is already running with PID {:?}",
                    existing.pid
                )));
            }
        }

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(argv);
        cmd.current_dir(&self.canonical_path);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            LocalIpcError::InternalRuntimeError(format!("Failed to spawn service '{name}': {e}"))
        })?;

        let pid = child.id().ok_or_else(|| {
            LocalIpcError::InternalRuntimeError("Failed to obtain child process ID".into())
        })?;

        let log_lines = Arc::new(RwLock::new(Vec::new()));
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

        if let Some(out) = child.stdout.take() {
            let logs = log_lines.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(out).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut buf = logs.write().await;
                    if buf.len() >= 5000 {
                        buf.remove(0);
                    }
                    buf.push(line);
                }
            });
        }

        if let Some(err) = child.stderr.take() {
            let logs = log_lines.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut reader = tokio::io::BufReader::new(err).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut buf = logs.write().await;
                    if buf.len() >= 5000 {
                        buf.remove(0);
                    }
                    buf.push(line);
                }
            });
        }

        let this_ws = Arc::downgrade(self);
        let name_clone = name.to_string();
        tokio::spawn(async move {
            tokio::select! {
                exit_status = child.wait() => {
                    if let Some(ws) = this_ws.upgrade() {
                        let exit_clean = exit_status.map(|s| s.success()).unwrap_or(false);
                        let final_state = if exit_clean { "stopped" } else { "crashed" };
                        ws.update_service_state(&name_clone, final_state, None).await;
                        let mut managed = ws.managed_processes.lock().await;
                        managed.remove(&name_clone);
                    }
                }
                _ = &mut stop_rx => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    if let Some(ws) = this_ws.upgrade() {
                        ws.update_service_state(&name_clone, "stopped", None).await;
                        let mut managed = ws.managed_processes.lock().await;
                        managed.remove(&name_clone);
                    }
                }
            }
        });

        let lease_id = omen_core::RuntimeLeaseId::generate();
        let managed_service = ManagedChildService {
            name: name.to_string(),
            command: command.to_string(),
            argv: argv.to_vec(),
            pid,
            lease_id: lease_id.clone(),
            started_at: std::time::Instant::now(),
            log_lines: log_lines.clone(),
            stop_tx: Some(stop_tx),
        };
        self.managed_processes
            .lock()
            .await
            .insert(name.to_string(), managed_service);

        self.service_configs
            .write()
            .await
            .insert(name.to_string(), (command.to_string(), argv.to_vec()));

        let info = ManagedServiceInfo {
            name: name.to_string(),
            resource_uri: format!("proc://workspace/{name}"),
            pid: Some(pid),
            command: format!("{} {}", command, argv.join(" ")),
            state: "running".to_string(),
            uptime_secs: 0,
            lease_id: Some(lease_id.to_string()),
        };
        self.register_service(info.clone()).await;
        Ok(info)
    }

    pub async fn stop_managed_service(&self, name: &str) -> Result<String, LocalIpcError> {
        let mut managed = self.managed_processes.lock().await;
        if let Some(mut proc) = managed.remove(name) {
            let pid = proc.pid;
            if let Some(stop_tx) = proc.stop_tx.take() {
                let _ = stop_tx.send(());
            }
            drop(managed);

            // Bounded wait up to 1000ms for graceful stop
            let start = std::time::Instant::now();
            while omen_engine::is_process_alive(pid) && start.elapsed().as_millis() < 1000 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }

            // If still alive after deadline, force terminate
            if omen_engine::is_process_alive(pid) {
                omen_engine::kill_process(pid);
            }

            self.update_service_state(name, "stopped", None).await;
            Ok(name.to_string())
        } else {
            drop(managed);
            let services = self.services.read().await;
            if let Some(svc) = services.get(name) {
                let pid = svc.pid;
                let state = svc.state.clone();
                drop(services);

                if state == "observed" {
                    if let Some(p) = pid
                        && omen_engine::is_process_alive(p)
                    {
                        Err(LocalIpcError::LocalPeerDenied(format!(
                            "Service '{name}' is observed with active PID {p}; daemon lacks process ownership to stop it safely",
                        )))
                    } else {
                        self.update_service_state(name, "stopped", None).await;
                        Ok(name.to_string())
                    }
                } else if state == "stopped" {
                    Ok(name.to_string())
                } else {
                    self.update_service_state(name, "stopped", None).await;
                    Ok(name.to_string())
                }
            } else {
                Err(LocalIpcError::ServiceNotFound(format!(
                    "Service '{name}' not found"
                )))
            }
        }
    }

    pub async fn restart_managed_service(
        self: &Arc<Self>,
        name: &str,
    ) -> Result<ManagedServiceInfo, LocalIpcError> {
        {
            let services = self.services.read().await;
            if let Some(svc) = services.get(name)
                && svc.state == "observed"
                && let Some(p) = svc.pid
                && omen_engine::is_process_alive(p)
            {
                return Err(LocalIpcError::LocalPeerDenied(format!(
                    "Service '{name}' is observed with active PID {p}; daemon cannot restart unowned process",
                )));
            }
        }

        let (cmd, argv) = {
            let configs = self.service_configs.read().await;
            if let Some((cmd, argv)) = configs.get(name) {
                (cmd.clone(), argv.clone())
            } else {
                let managed = self.managed_processes.lock().await;
                if let Some(proc) = managed.get(name) {
                    (proc.command.clone(), proc.argv.clone())
                } else {
                    return Err(LocalIpcError::ServiceNotFound(format!(
                        "Service '{name}' not found"
                    )));
                }
            }
        };

        let _ = self.stop_managed_service(name).await;
        self.start_managed_service(name, &cmd, &argv).await
    }

    pub async fn service_logs(
        &self,
        name: &str,
        tail_lines: usize,
    ) -> Result<(Vec<String>, Option<String>), LocalIpcError> {
        let managed = self.managed_processes.lock().await;
        if let Some(child) = managed.get(name) {
            let logs = child.log_lines.read().await;
            let total = logs.len();
            let start = total.saturating_sub(tail_lines);
            let lines = logs[start..].to_vec();

            let full_text = logs.join("\n");
            let cas_uri = if full_text.len() > 8192 {
                let mut db = self.db.lock().await;
                self.cas
                    .store(
                        &mut db,
                        full_text.as_bytes(),
                        "text/plain",
                        name,
                        omen_core::RetentionClass::Ephemeral,
                    )
                    .ok()
                    .map(|m| m.uri.to_string())
            } else {
                None
            };

            Ok((lines, cas_uri))
        } else if self.services.read().await.contains_key(name) {
            Ok((
                vec!["Service is stopped; no active process stream".to_string()],
                None,
            ))
        } else {
            Err(LocalIpcError::ServiceNotFound(format!(
                "Service '{name}' not found"
            )))
        }
    }

    pub async fn register_service(&self, info: ManagedServiceInfo) {
        let name = info.name.clone();
        let state = info.state.clone();
        let pid = info.pid;
        let command = info.command.clone();

        {
            let mut services = self.services.write().await;
            services.insert(name.clone(), info);
        }

        {
            let db = self.db.lock().await;
            let _ = WorkspacePersistence::upsert_service(
                &db,
                &ServiceRecord {
                    workspace_id: self.workspace_id.clone(),
                    name: name.clone(),
                    command,
                    pid,
                    state: state.clone(),
                    started_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            );
        }

        self.broadcast_event(EventPayload::ServiceStateChanged { name, state, pid });
    }

    pub async fn update_service_state(&self, name: &str, state: &str, pid: Option<u32>) -> bool {
        let mut services = self.services.write().await;
        if let Some(svc) = services.get_mut(name) {
            svc.state = state.to_string();
            svc.pid = pid;
            let svc_name = svc.name.clone();
            let svc_state = svc.state.clone();
            let svc_cmd = svc.command.clone();

            drop(services);

            {
                let db = self.db.lock().await;
                let _ = WorkspacePersistence::upsert_service(
                    &db,
                    &ServiceRecord {
                        workspace_id: self.workspace_id.clone(),
                        name: svc_name.clone(),
                        command: svc_cmd,
                        pid,
                        state: svc_state.clone(),
                        started_at: chrono::Utc::now().to_rfc3339(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
            }

            self.broadcast_event(EventPayload::ServiceStateChanged {
                name: svc_name,
                state: svc_state,
                pid,
            });
            true
        } else {
            false
        }
    }

    /// Repair 3: the persistence result is returned, never discarded.
    /// Callers on the dispatch path refuse physical work when this fails;
    /// callers after physical work surface an explicit persistence failure
    /// instead of claiming durable truth.
    pub async fn record_request_receipt(
        &self,
        req_id: &str,
        exec_id: Option<&str>,
        status: &str,
    ) -> Result<(), CoreError> {
        self.record_request_receipt_staged(ReceiptStage::Other, req_id, exec_id, status)
            .await
    }

    async fn record_request_receipt_staged(
        &self,
        stage: ReceiptStage,
        req_id: &str,
        exec_id: Option<&str>,
        status: &str,
    ) -> Result<(), CoreError> {
        match (stage, self.persistence_failpoint()) {
            (ReceiptStage::Running, PersistenceFailpoint::FailRunningReceipt) => {
                return Err(CoreError::Internal(
                    "injected persistence failure: pre-dispatch Running receipt refused"
                        .to_string(),
                ));
            }
            (ReceiptStage::Terminal, PersistenceFailpoint::FailTerminalReceipt) => {
                return Err(CoreError::Internal(
                    "injected persistence failure: terminal receipt refused".to_string(),
                ));
            }
            _ => {}
        }
        let db = self.db.lock().await;
        WorkspacePersistence::record_request_receipt(
            &db,
            &RequestReceiptRecord {
                consequential_request_id: req_id.to_string(),
                execution_id: exec_id.map(Into::into),
                status: status.to_string(),
                recorded_at: chrono::Utc::now().to_rfc3339(),
            },
        )
    }

    pub async fn query_request_receipt(
        &self,
        req_id: &str,
    ) -> Result<Option<RequestReceiptRecord>, CoreError> {
        if self.poll_read_failpoint(true) {
            return Err(CoreError::Internal(
                "injected persistence failure: durable request-receipt read refused".to_string(),
            ));
        }
        let db = self.db.lock().await;
        WorkspacePersistence::get_request_receipt(&db, req_id)
    }

    /// Repair Preview 12: durable execution-ID receipt lookup that preserves
    /// read failure instead of collapsing it into absence. `Ok(None)` is
    /// positive proof no receipt exists; `Err` means Omen could not
    /// establish whether one exists — callers must fail closed.
    pub async fn query_receipt_by_execution_id(
        &self,
        execution_id: &str,
    ) -> Result<Option<RequestReceiptRecord>, CoreError> {
        if self.poll_read_failpoint(false) {
            return Err(CoreError::Internal(
                "injected persistence failure: durable execution-ID receipt read refused"
                    .to_string(),
            ));
        }
        let db = self.db.lock().await;
        WorkspacePersistence::get_receipt_by_execution_id(&db, execution_id)
    }

    /// Repair Preview 12: consult the read-failure failpoint. `by_request`
    /// selects which durable lookup is being attempted. Returns true when
    /// the read must fail. Production runs `Off` (and an exhausted
    /// `FailReceiptReadAfterSuccesses(0)`), so the real lookup proceeds.
    fn poll_read_failpoint(&self, by_request: bool) -> bool {
        let mut slot = match self.persistence_failpoint.lock() {
            Ok(slot) => slot,
            Err(_) => return false,
        };
        match *slot {
            PersistenceFailpoint::FailRequestReceiptRead if by_request => true,
            PersistenceFailpoint::FailReceiptByExecRead if !by_request => true,
            PersistenceFailpoint::FailReceiptReadAfterSuccesses(remaining) => {
                if remaining == 0 {
                    true
                } else {
                    *slot = PersistenceFailpoint::FailReceiptReadAfterSuccesses(remaining - 1);
                    false
                }
            }
            _ => false,
        }
    }

    /// E2 stop truth: request cancellation of a live brokered execution.
    ///
    /// Intent and proof are strictly separated:
    /// - firing the live flag records `CancellationRequested` (intent);
    /// - the broker task still performs the physical tree-stop and observes
    ///   the child; only observed death becomes `TerminationConfirmed`;
    /// - a naturally finished execution reports `AlreadyFinished` with its
    ///   terminal receipt status — cancel never rewrites a completed outcome;
    /// - no live handle and no terminal receipt (restart/crash boundary)
    ///   reports `OutcomeUnknown` and reconciles the receipt to `Unknown`.
    ///
    /// The observation wait is bounded (`CANCEL_OBSERVE_TIMEOUT`); expiry
    /// also reports `OutcomeUnknown` rather than hanging the caller.
    /// Subscribe to the terminal broadcast for `dedup_id`, if a broker task
    /// is still holding its `in_flight` entry. A double-checked subscribe
    /// covers a task completing between the check and the subscription.
    async fn subscribe_cancel_terminal(
        &self,
        dedup_id: &str,
    ) -> Option<tokio::sync::broadcast::Receiver<Result<ExecutionResultSummary, LocalIpcError>>>
    {
        {
            let in_flight = self.in_flight_executions.lock().await;
            if let Some(tx) = in_flight.get(dedup_id) {
                return Some(tx.subscribe());
            }
        }
        tokio::task::yield_now().await;
        let in_flight = self.in_flight_executions.lock().await;
        in_flight.get(dedup_id).map(|tx| tx.subscribe())
    }

    /// Await an observed terminal broadcast, mapping it to a cancel report.
    /// Returns `None` only when the broadcast was lost (senders dropped
    /// without a value): the caller must fall through to the durable
    /// receipt instead of manufacturing an outcome. A timed-out wait
    /// reports `OutcomeUnknown` directly — the flag was fired and the task
    /// did not resolve within the bound.
    async fn await_cancel_terminal(
        &self,
        execution_id: &str,
        mut sub: tokio::sync::broadcast::Receiver<Result<ExecutionResultSummary, LocalIpcError>>,
    ) -> Option<CancelRecord> {
        let mk = |outcome: CancelOutcome, detail: &str| CancelRecord {
            execution_id: execution_id.to_string(),
            outcome,
            detail: detail.to_string(),
        };
        match tokio::time::timeout(CANCEL_OBSERVE_TIMEOUT, sub.recv()).await {
            Ok(Ok(Ok(summary))) => Some(match summary.runtime_status {
                // Repair 2: prevention of execution is not proof of
                // termination. The typed `dispatch_prevented` flag comes
                // from the engine's CANCELLED_BEFORE_DISPATCH signal, never
                // from prose: no spawn means no tree-stop and no observed
                // death, so TerminationConfirmed would be a lie here.
                omen_core::RuntimeStatus::Cancelled if summary.dispatch_prevented => mk(
                    CancelOutcome::DispatchPrevented,
                    "stop arrived before physical dispatch: no process was spawned, no tree-stop was issued, no physical death was observed",
                ),
                omen_core::RuntimeStatus::Cancelled => mk(
                    CancelOutcome::TerminationConfirmed,
                    "stop requested, tree-stop issued, physical death observed",
                ),
                omen_core::RuntimeStatus::OutcomeUnknown => mk(
                    CancelOutcome::OutcomeUnknown,
                    "stop requested but physical death could not be confirmed",
                ),
                _ => mk(
                    CancelOutcome::AlreadyFinished {
                        terminal_status: format!("{:?}", summary.runtime_status),
                    },
                    "execution reached a terminal state before the stop took effect",
                ),
            }),
            Ok(Ok(Err(_))) | Ok(Err(_)) => {
                // Task error or lost broadcast: the durable receipt (written
                // before any broadcast) is the surviving truth. Fall through.
                None
            }
            Err(_) => Some(mk(
                CancelOutcome::OutcomeUnknown,
                "stop requested but the terminal outcome was not observable within the bounded wait",
            )),
        }
    }

    /// Report from a durable receipt. Terminal receipts speak for
    /// themselves; anything else with no live task behind it is a
    /// restart/crash boundary and reconciles to `Unknown`.
    async fn report_receipt_for_cancel(
        &self,
        execution_id: &str,
        rec: RequestReceiptRecord,
    ) -> CancelRecord {
        let mk = |outcome: CancelOutcome, detail: &str| CancelRecord {
            execution_id: execution_id.to_string(),
            outcome,
            detail: detail.to_string(),
        };
        match rec.status.as_str() {
            "Completed" | "Failed" | "Cancelled" => mk(
                CancelOutcome::AlreadyFinished {
                    terminal_status: rec.status,
                },
                "execution already reached a terminal state",
            ),
            "Unknown" => mk(
                CancelOutcome::OutcomeUnknown,
                "execution outcome is already unknown (restart/crash boundary)",
            ),
            _ => {
                // No live task behind a non-terminal receipt: restart/crash
                // boundary. Reconcile to Unknown. If even the reconcile
                // write fails, the receipt stays non-terminal (conservative:
                // future submits fail closed again) and the report says so —
                // the Unknown outcome itself comes from the absence of any
                // live owner, not from the failed write.
                let reconciled = self
                    .record_request_receipt(
                        &rec.consequential_request_id,
                        Some(execution_id),
                        "Unknown",
                    )
                    .await
                    .is_ok();
                mk(
                    CancelOutcome::OutcomeUnknown,
                    if reconciled {
                        "no live execution task; receipt reconciled to Unknown"
                    } else {
                        "no live execution task; receipt reconcile persistence failed so the identity stays non-terminal (fail closed); outcome Unknown from absence of a live owner"
                    },
                )
            }
        }
    }

    /// E2 stop truth: request cancellation of a live brokered execution.
    ///
    /// Intent and proof are strictly separated:
    /// - firing the live flag records `CancellationRequested` (intent);
    /// - the broker task still performs the physical tree-stop and observes
    ///   the child; only observed death becomes `TerminationConfirmed`;
    /// - a naturally finished execution reports `AlreadyFinished` with its
    ///   terminal receipt status — cancel never rewrites a completed outcome;
    /// - no live task and no terminal receipt (restart/crash boundary)
    ///   reports `OutcomeUnknown` and reconciles the receipt to `Unknown`.
    ///
    /// Race discipline: a live broker task always holds its `in_flight`
    /// broadcast entry until AFTER the terminal receipt is written, so a
    /// missed broadcast implies the receipt went terminal — the receipt is
    /// re-read rather than trusted from a potentially transient observation.
    /// The observation wait is bounded (`CANCEL_OBSERVE_TIMEOUT`); expiry
    /// reports `OutcomeUnknown` rather than hanging the caller.
    pub async fn cancel_execution(&self, execution_id: &str) -> CancelRecord {
        let mk = |outcome: CancelOutcome, detail: &str| CancelRecord {
            execution_id: execution_id.to_string(),
            outcome,
            detail: detail.to_string(),
        };

        // Live in-flight execution: record intent, fire the flag, observe.
        // Order matters: the receipt write precedes the fire so status
        // queries can observe CancellationRequested; the fire precedes the
        // subscribe so a completing task cannot slip between them (a missed
        // broadcast then implies the terminal receipt write already landed).
        let live = {
            let switches = self.cancel_switches.lock().await;
            let index = self.cancel_index.lock().await;
            match (
                switches.get(execution_id).cloned(),
                index.get(execution_id).cloned(),
            ) {
                (Some(tx), Some(dedup_id)) => Some((tx, dedup_id)),
                _ => None,
            }
        };
        if let Some((cancel_tx, dedup_id)) = live {
            // Intent annotation: best-effort is principled here, not a
            // discarded error. The stop flag is still fired and the terminal
            // outcome still comes from live observation plus the broker
            // task's own fail-closed persistence (Repair 3), which surfaces
            // explicitly to the submitter. An intent-write failure therefore
            // annotates the report instead of rewriting observed truth.
            let intent_persisted = self
                .record_request_receipt(&dedup_id, Some(execution_id), "CancellationRequested")
                .await
                .is_ok();
            let _ = cancel_tx.send(true);
            if let Some(sub) = self.subscribe_cancel_terminal(&dedup_id).await
                && let Some(mut report) = self.await_cancel_terminal(execution_id, sub).await
            {
                if !intent_persisted {
                    report.detail.push_str("; WARNING: cancellation-intent receipt persistence failed (observed outcome stands; the broker task reports its own durability explicitly)");
                }
                return report;
            }
            // Unobservable broadcast: fall through to the durable receipt.
        }

        // Durable receipt decides. A task that is still alive holds its
        // in_flight entry until after its terminal receipt write, so prefer
        // live observation for non-terminal receipts; otherwise report.
        // Repair Preview 12: read failure is NOT absence — a failed lookup
        // can never become `NotFound`. Only a successful read returning no
        // row proves the execution is unknown to the daemon.
        let receipt = match self.query_receipt_by_execution_id(execution_id).await {
            Ok(receipt) => receipt,
            Err(read_error) => {
                return mk(
                    CancelOutcome::OutcomeUnknown,
                    &format!(
                        "durable receipt lookup failed; Omen cannot establish whether this execution is known or terminal: {read_error}"
                    ),
                );
            }
        };
        match receipt {
            None => mk(
                CancelOutcome::NotFound,
                "no execution with this ID is known to the daemon",
            ),
            Some(rec) => match rec.status.as_str() {
                "Completed" | "Failed" | "Cancelled" | "Unknown" => {
                    self.report_receipt_for_cancel(execution_id, rec).await
                }
                _ => {
                    // Non-terminal receipt: the task may still be alive
                    // (switches cleaned just ahead of us). Prefer live
                    // observation; a missed broadcast implies the terminal
                    // receipt write landed, so re-read rather than trusting
                    // the transient observation.
                    if let Some(sub) = self
                        .subscribe_cancel_terminal(&rec.consequential_request_id)
                        .await
                        && let Some(report) = self.await_cancel_terminal(execution_id, sub).await
                    {
                        return report;
                    }
                    // Repair Preview 12: a reread failure is NOT a vanishing
                    // execution — the identity is preserved, only the
                    // observation failed. Never `NotFound`, never "vanished".
                    let reread = match self.query_receipt_by_execution_id(execution_id).await {
                        Ok(reread) => reread,
                        Err(read_error) => {
                            return mk(
                                CancelOutcome::OutcomeUnknown,
                                &format!(
                                    "durable receipt reread failed after live observation lapsed; Omen cannot establish terminal truth — the execution has NOT vanished and its identity is preserved: {read_error}"
                                ),
                            );
                        }
                    };
                    match reread {
                        Some(fresh) => self.report_receipt_for_cancel(execution_id, fresh).await,
                        None => mk(
                            CancelOutcome::NotFound,
                            "execution vanished between observations",
                        ),
                    }
                }
            },
        }
    }

    pub fn db(&self) -> Arc<Mutex<Database>> {
        self.db.clone()
    }

    pub async fn record_history(
        &self,
        session_id: &str,
        command: &str,
        exit_code: Option<i32>,
        duration_ms: u64,
        stdout_artifact: Option<String>,
        stderr_artifact: Option<String>,
    ) -> Result<String, CoreError> {
        self.record_history_with_id(
            None,
            session_id,
            command,
            exit_code,
            duration_ms,
            stdout_artifact,
            stderr_artifact,
            None,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_history_with_id(
        &self,
        custom_execution_id: Option<ExecutionId>,
        session_id: &str,
        command: &str,
        exit_code: Option<i32>,
        duration_ms: u64,
        stdout_artifact: Option<String>,
        stderr_artifact: Option<String>,
        runtime: Option<omen_core::RuntimeStatus>,
        dispatch_prevented: bool,
    ) -> Result<String, CoreError> {
        let (execution_id, exec_id_str) = if let Some(id) = custom_execution_id {
            let s = id.to_string();
            (id, s)
        } else {
            let id = ExecutionId::generate();
            let s = id.to_string();
            (id, s)
        };
        let session_id_typed = InteractiveSessionId::new(session_id)?;

        // E2 history truth: the broker always stamps the coarse history
        // status explicitly via the single RuntimeStatus::history_label
        // authority, so every recording surface projects identical truth.
        // Repair A: the stamp is a TYPED envelope carrying the exact
        // physical runtime verbatim alongside the coarse label — coarse
        // history status != exact RuntimeStatus, and replay uses the exact
        // field, never the label.
        let envelope_json = runtime.map(|status| {
            HistoryStatusEnvelope::broker(status, exit_code, dispatch_prevented).to_json()
        });

        let record = ExecutionRecord {
            execution_id: execution_id.clone(),
            session_id: session_id_typed,
            command: command.to_string(),
            exit_code,
            duration_ms: Some(duration_ms as i64),
            stdout_artifact,
            stderr_artifact,
            envelope_json,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        {
            let mut db = self.db.lock().await;
            ExecutionHistory::record_execution(&mut db, &record, &[], &[], &[])?;
        }

        self.broadcast_event(EventPayload::ExecutionRecorded {
            execution_id: exec_id_str.clone(),
            session_id: session_id.to_string(),
            command: command.to_string(),
            exit_code,
        });

        Ok(exec_id_str)
    }

    pub async fn get_last_execution(
        &self,
        session_id: &str,
    ) -> Result<Option<ExecutionRecord>, CoreError> {
        let sid = InteractiveSessionId::new(session_id)?;
        let db = self.db.lock().await;
        ExecutionHistory::get_last_execution(&db, &sid)
    }

    pub fn cas(&self) -> Arc<ContentAddressedStore> {
        self.cas.clone()
    }

    pub fn supervisor(&self) -> Arc<ProcessSupervisor> {
        self.supervisor.clone()
    }

    /// Release a claimed in-flight slot, delivering the resolution to any
    /// submitter that coalesced onto us between the claim and the release.
    /// No lock is held across await: send is synchronous.
    async fn release_claimed_slot(
        &self,
        dedup_id: &str,
        resolution: Result<ExecutionResultSummary, LocalIpcError>,
    ) {
        let mut in_flight = self.in_flight_executions.lock().await;
        if let Some(tx) = in_flight.remove(dedup_id) {
            let _ = tx.send(resolution);
        }
    }

    /// Repair 1: resolve a durable receipt with no silent fall-through to
    /// dispatch. The durable consequential receipt is the authority across
    /// restart — a consumed identity never becomes eligible for physical
    /// work again:
    /// - `Completed` → replay the recorded terminal truth;
    /// - `Cancelled` → replay the recorded cancellation truth (never
    ///   resurrect a cancelled operation);
    /// - `Failed` → explicit terminal-failure refusal (never silently
    ///   retry physical work);
    /// - `Unknown` → fail closed;
    /// - anything else (`Running`, `CancellationRequested`, unexpected) →
    ///   no live owner can exist (the caller claimed the only in-flight
    ///   slot), so reconcile to `Unknown` and fail closed.
    ///
    /// Zero spawn on every path.
    async fn resolve_existing_receipt(
        &self,
        dedup_id: &str,
        receipt: &RequestReceiptRecord,
    ) -> Result<ExecutionResultSummary, LocalIpcError> {
        match receipt.status.as_str() {
            "Unknown" => Err(LocalIpcError::ExecutionStatusUnknown(dedup_id.to_string())),
            "Completed" => {
                self.replay_terminal_receipt(dedup_id, receipt, "Completed")
                    .await
            }
            "Cancelled" => {
                self.replay_terminal_receipt(dedup_id, receipt, "Cancelled")
                    .await
            }
            "Failed" => Err(LocalIpcError::RequestDuplicate(format!(
                "consequential request '{dedup_id}' already reached terminal status 'Failed' (execution {}); refusing re-execution",
                receipt
                    .execution_id
                    .as_deref()
                    .unwrap_or("no execution recorded")
            ))),
            _ => {
                // Non-terminal receipt with no live owner behind it:
                // restart/crash/panic boundary. Reconcile to Unknown and
                // fail closed. A reconcile-write failure is explicit — and
                // still never dispatches.
                match self
                    .record_request_receipt(dedup_id, receipt.execution_id.as_deref(), "Unknown")
                    .await
                {
                    Ok(()) => Err(LocalIpcError::ExecutionStatusUnknown(dedup_id.to_string())),
                    Err(persistence) => Err(LocalIpcError::PersistenceFailure {
                        execution_id: receipt
                            .execution_id
                            .clone()
                            .unwrap_or_else(|| format!("request:{dedup_id}")),
                        stage: "reconcile_receipt".to_string(),
                        physical_outcome: "no physical work started".to_string(),
                        detail: persistence.to_string(),
                    }),
                }
            }
        }
    }

    /// Reproduce a recorded terminal outcome for a consumed consequential
    /// ID. A receipt whose history cannot be read is refused, never
    /// re-executed: absence of readable truth is not a license to dispatch.
    async fn replay_terminal_receipt(
        &self,
        dedup_id: &str,
        receipt: &RequestReceiptRecord,
        receipt_status: &str,
    ) -> Result<ExecutionResultSummary, LocalIpcError> {
        let refused = |why: String| {
            LocalIpcError::InternalRuntimeError(format!(
                "durable {receipt_status} receipt for consequential request '{dedup_id}' {why}; refusing new dispatch"
            ))
        };
        let exec_id = receipt
            .execution_id
            .clone()
            .ok_or_else(|| refused("carries no execution identity".to_string()))?;
        let eid = ExecutionId::new(&exec_id)
            .map_err(|_| refused("carries an unreadable execution identity".to_string()))?;
        let rec = {
            let db = self.db.lock().await;
            match ExecutionHistory::get_execution(&db, &eid) {
                Ok(Some(rec)) => rec,
                // Repair Preview 12: the history-read error text is preserved
                // in the refusal instead of being blurred into absence. Every
                // path below still refuses redispatch — zero replay without
                // exact readable truth.
                Ok(None) => return Err(refused("has no readable history".to_string())),
                Err(history_error) => {
                    return Err(refused(format!(
                        "history read failed so exact runtime truth cannot be established ({history_error})"
                    )));
                }
            }
        };
        // E2 replay truth: reproduce the recorded EXACT runtime, never a
        // reconstruction from the coarse label. Legacy rows without exact
        // information use the narrow safe fallback; ambiguous legacy rows
        // refuse replay (the identity stays consumed — refusal never
        // re-arms physical execution).
        let runtime_status = replay_runtime_status(&rec, receipt_status)
            .ok_or_else(|| refused("has no exact runtime truth to replay".to_string()))?;
        let dispatch_prevented = recorded_dispatch_prevented(&rec);
        let summary = ExecutionResultSummary {
            execution_id: exec_id,
            runtime_status,
            exit_code: rec.exit_code,
            duration_ms: rec.duration_ms.unwrap_or(0) as u64,
            stdout_preview: rec.command.clone(),
            stderr_preview: String::new(),
            stdout_artifact: rec.stdout_artifact,
            stderr_artifact: rec.stderr_artifact,
            dispatch_prevented,
        };
        self.execution_cache
            .write()
            .await
            .insert(dedup_id.to_string(), summary.clone());
        Ok(summary)
    }

    /// Repair 3: end a broker task terminally WITHOUT caching a success
    /// and without advancing the receipt. Used when durability failed
    /// after physical work: the receipt stays non-terminal so the identity
    /// can never dispatch again, while coalesced subscribers still get
    /// control back with the explicit failure.
    async fn fail_task_terminal(
        &self,
        dedup_id: &str,
        err: Result<ExecutionResultSummary, LocalIpcError>,
    ) {
        debug_assert!(err.is_err());
        let mut in_flight = self.in_flight_executions.lock().await;
        if let Some(tx) = in_flight.remove(dedup_id) {
            let _ = tx.send(err);
        }
    }

    pub async fn execute_broker(
        self: &Arc<Self>,
        params: BrokerExecutionParams<'_>,
    ) -> Result<ExecutionResultSummary, LocalIpcError> {
        let dedup_id = params.dedup_id;
        let session_id = params.session_id;
        let tool = params.tool;
        let operation = params.operation;
        let args = params.args;
        let cwd = params.cwd;
        let timeout_ms = params.timeout_ms;

        // 1. Check if already completed in memory cache
        if let Some(cached) = self.execution_cache.read().await.get(dedup_id) {
            return Ok(cached.clone());
        }

        // 2. Claim the in-flight slot or coalesce onto a live owner. From
        // here, exactly one task owns the dispatch decision for this
        // consequential ID; every concurrent submitter observes our
        // resolution through the broadcast instead of dispatching again.
        let maybe_sub = {
            let mut in_flight = self.in_flight_executions.lock().await;
            if let Some(tx) = in_flight.get(dedup_id) {
                Some(tx.subscribe())
            } else {
                let (tx, _) = broadcast::channel(4);
                in_flight.insert(dedup_id.to_string(), tx);
                None
            }
        };

        if let Some(mut subscriber) = maybe_sub {
            return match subscriber.recv().await {
                Ok(res) => res,
                Err(e) => Err(LocalIpcError::InternalRuntimeError(format!(
                    "In-flight execution subscription error: {e}"
                ))),
            };
        }

        // We claimed a fresh slot. Resolve the durable receipt BEFORE any
        // physical work. Repair 1 invariant: a consequential request ID may
        // create physical work only when no durable receipt for that ID
        // exists. The receipt check happens AFTER the claim so a terminal
        // write landing concurrently is resolved (replay/refuse) rather
        // than raced into a duplicate dispatch; subscribers that attached
        // to our fresh slot receive the same resolution.
        //
        // Repair Preview 12: `Ok(None)` is positive proof of absence and the
        // ONLY path toward dispatch. `Err` means Omen failed to establish
        // whether a receipt exists — unknown is not absent — so the broker
        // fails closed with an explicit `PersistenceFailure`: zero dispatch,
        // zero new identity, zero overwrite, zero history. The claimed slot
        // is released so every coalesced subscriber receives the same
        // failure and no future submitter strands.
        match self.query_request_receipt(dedup_id).await {
            Ok(Some(receipt)) => {
                let resolution = self.resolve_existing_receipt(dedup_id, &receipt).await;
                self.release_claimed_slot(dedup_id, resolution.clone())
                    .await;
                return resolution;
            }
            Ok(None) => {}
            Err(read_error) => {
                let refusal = Err(LocalIpcError::PersistenceFailure {
                    // No execution identity was minted on this path — and
                    // none is persisted — so there is nothing truthful to
                    // name here. The consequential request ID is carried in
                    // the detail instead.
                    execution_id: "<none>".to_string(),
                    stage: "receipt_read".to_string(),
                    physical_outcome: "no physical work started".to_string(),
                    detail: format!(
                        "durable receipt lookup for consequential request '{dedup_id}' failed; Omen cannot establish whether a receipt exists, so absence is not assumed: {read_error}"
                    ),
                });
                self.release_claimed_slot(dedup_id, refusal.clone()).await;
                return refusal;
            }
        }

        let execution_id = ExecutionId::generate();
        let exec_id_str = execution_id.to_string();

        // 3. Construct argv (pure validation input; no side effects yet).
        let mut argv = Vec::new();
        if !tool.is_empty() && tool != "exec" {
            argv.push(tool.to_string());
        }
        if !operation.is_empty() {
            argv.push(operation.to_string());
        }
        argv.extend(args.iter().cloned());
        if argv.is_empty() && tool == "exec" && !args.is_empty() {
            argv = args.to_vec();
        }
        if argv.is_empty() {
            // Malformed requests never reach physical work. The Failed
            // annotation is best-effort: the refusal is deterministic and
            // reproducible, so a lost annotation simply re-validates on
            // retry instead of dispatching anything.
            let _ = self
                .record_request_receipt_staged(
                    ReceiptStage::Terminal,
                    dedup_id,
                    Some(&exec_id_str),
                    "Failed",
                )
                .await;
            let refusal = Err(LocalIpcError::MalformedRequest(
                "Command argv cannot be empty".into(),
            ));
            self.release_claimed_slot(dedup_id, refusal.clone()).await;
            return refusal;
        }

        // 4. Pre-dispatch: the Running receipt MUST persist or nothing
        // spawns (Repair 3). No durable identity means no dispatch — this
        // is the point where fail-closed is cheap.
        if let Err(persistence) = self
            .record_request_receipt_staged(
                ReceiptStage::Running,
                dedup_id,
                Some(&exec_id_str),
                "Running",
            )
            .await
        {
            let refusal = Err(LocalIpcError::PersistenceFailure {
                execution_id: exec_id_str,
                stage: "running_receipt".to_string(),
                physical_outcome: "no physical work started".to_string(),
                detail: persistence.to_string(),
            });
            self.release_claimed_slot(dedup_id, refusal.clone()).await;
            return refusal;
        }

        let this = Arc::clone(self);
        let dedup_id_str = dedup_id.to_string();
        let session_id_str = session_id.to_string();
        let tool_str = tool.to_string();
        let exec_cwd = if cwd.is_empty() {
            self.canonical_path.clone()
        } else {
            PathBuf::from(cwd)
        };

        // E2 stop truth: live cancellation flag for this execution. Firing
        // it records intent; the task below still performs the physical stop
        // and observes the outcome (confirm or unknown).
        let (cancel_tx, cancel_rx) = watch::channel(false);
        {
            let mut switches = self.cancel_switches.lock().await;
            switches.insert(exec_id_str.clone(), cancel_tx);
            let mut index = self.cancel_index.lock().await;
            index.insert(exec_id_str.clone(), dedup_id_str.clone());
        }

        let join_handle = tokio::spawn(async move {
            // Repair 2 test seam: park here — after the live cancel switch
            // is registered, before any backend contact — so a test can
            // deterministically fire a cancel pre-dispatch. Production
            // never arms the gate.
            if let Some(gate) = this.prespawn_gate_snapshot() {
                gate.task_parked.notify_one();
                gate.release.notified().await;
            }

            let req = ExecutionRequest {
                argv: argv.clone(),
                cwd: exec_cwd,
                env: vec![],
                stdin_mode: omen_core::StdioMode::Closed,
                stdin_payload: None,
                timeout_ms: if timeout_ms == 0 { 60000 } else { timeout_ms },
                inline_budget: 8192,
                required_assurance: omen_core::RequiredAssurance::default(),
                secrets: vec![],
            };

            let exec_result = this.supervisor.execute_cancelable(req, cancel_rx).await;
            // The execution is terminal: release the cancellation switch.
            {
                let mut switches = this.cancel_switches.lock().await;
                switches.remove(&exec_id_str);
                let mut index = this.cancel_index.lock().await;
                index.remove(&exec_id_str);
            }
            match exec_result {
                Ok(output) => {
                    let cmd_str = argv.join(" ");

                    // Check CAS offload for large output
                    let (stdout_art, stderr_art) = {
                        let mut db = this.db.lock().await;
                        let so_art = if !output.stdout_all.is_empty() {
                            this.cas
                                .store(
                                    &mut db,
                                    &output.stdout_all,
                                    "text/plain",
                                    &tool_str,
                                    omen_core::RetentionClass::Ephemeral,
                                )
                                .ok()
                                .map(|m| m.uri.to_string())
                        } else {
                            None
                        };
                        let se_art = if !output.stderr_all.is_empty() {
                            this.cas
                                .store(
                                    &mut db,
                                    &output.stderr_all,
                                    "text/plain",
                                    &tool_str,
                                    omen_core::RetentionClass::Ephemeral,
                                )
                                .ok()
                                .map(|m| m.uri.to_string())
                        } else {
                            None
                        };
                        (so_art, se_art)
                    };

                    let exit_code = output.process_exit.code;
                    let duration_ms = output.duration_ms;
                    let stdout_preview = output.stdout_sanitized();
                    let stderr_preview = output.stderr_sanitized();
                    // Repair 2: typed dispatch truth from the execution
                    // path. The engine sets CANCELLED_BEFORE_DISPATCH only
                    // on the zero-spawn pre-dispatch path; anything else
                    // that reports Cancelled went through a real spawn.
                    let dispatch_prevented =
                        output.process_exit.signal.as_deref() == Some("CANCELLED_BEFORE_DISPATCH");

                    // E2 terminal truth: the broker receipt distinguishes a
                    // confirmed stop from natural completion and from an
                    // unconfirmed outcome. The task owns this terminal write;
                    // `cancel_execution` only records intent and observes.
                    let terminal_receipt = match output.runtime_status {
                        omen_core::RuntimeStatus::Cancelled => "Cancelled",
                        omen_core::RuntimeStatus::OutcomeUnknown => "Unknown",
                        _ => "Completed",
                    };
                    let history_runtime = Some(output.runtime_status);
                    let physical_outcome = format!(
                        "exit={exit_code:?} runtime={:?} dispatch_prevented={dispatch_prevented}",
                        output.runtime_status
                    );

                    // Repair 3, post-execution rule: physical work already
                    // happened, so a history persistence failure must NOT
                    // pretend nothing happened and must NOT rewrite the
                    // physical result. The receipt is left non-terminal
                    // (never re-executable), and the failure surfaces with
                    // the execution identity plus the physical outcome.
                    // The history failpoint is consulted here rather than
                    // inside record_history_with_id so non-broker history
                    // surfaces keep their own behavior.
                    if this.persistence_failpoint() == PersistenceFailpoint::FailHistoryWrite {
                        let err = LocalIpcError::PersistenceFailure {
                            execution_id: exec_id_str.clone(),
                            stage: "history".to_string(),
                            physical_outcome,
                            detail: "injected persistence failure: terminal history refused"
                                .to_string(),
                        };
                        this.fail_task_terminal(&dedup_id_str, Err(err.clone()))
                            .await;
                        return Err(err);
                    }
                    if let Err(history_error) = this
                        .record_history_with_id(
                            Some(execution_id),
                            &session_id_str,
                            &cmd_str,
                            exit_code,
                            duration_ms,
                            stdout_art.clone(),
                            stderr_art.clone(),
                            history_runtime,
                            dispatch_prevented,
                        )
                        .await
                    {
                        let err = LocalIpcError::PersistenceFailure {
                            execution_id: exec_id_str.clone(),
                            stage: "history".to_string(),
                            physical_outcome,
                            detail: history_error.to_string(),
                        };
                        this.fail_task_terminal(&dedup_id_str, Err(err.clone()))
                            .await;
                        return Err(err);
                    }

                    // Repair 3: a terminal-receipt failure after history
                    // succeeded preserves the recorded history truth and
                    // leaves the receipt non-terminal, so the identity can
                    // never dispatch again — future submits reconcile to
                    // Unknown and fail closed.
                    if let Err(receipt_error) = this
                        .record_request_receipt_staged(
                            ReceiptStage::Terminal,
                            &dedup_id_str,
                            Some(&exec_id_str),
                            terminal_receipt,
                        )
                        .await
                    {
                        let err = LocalIpcError::PersistenceFailure {
                            execution_id: exec_id_str.clone(),
                            stage: "terminal_receipt".to_string(),
                            physical_outcome,
                            detail: receipt_error.to_string(),
                        };
                        this.fail_task_terminal(&dedup_id_str, Err(err.clone()))
                            .await;
                        return Err(err);
                    }

                    let summary = ExecutionResultSummary {
                        execution_id: exec_id_str,
                        runtime_status: output.runtime_status,
                        exit_code,
                        duration_ms,
                        stdout_preview,
                        stderr_preview,
                        stdout_artifact: stdout_art,
                        stderr_artifact: stderr_art,
                        dispatch_prevented,
                    };

                    // Cache in memory
                    this.execution_cache
                        .write()
                        .await
                        .insert(dedup_id_str.clone(), summary.clone());

                    // Notify in-flight subscribers
                    let mut in_flight = this.in_flight_executions.lock().await;
                    if let Some(tx) = in_flight.remove(&dedup_id_str) {
                        let _ = tx.send(Ok(summary.clone()));
                    }

                    Ok(summary)
                }
                Err(e) => {
                    // Spawn/validation failure inside the task: record the
                    // terminal Failed receipt. Repair A.1: the execution ID
                    // was already durably minted on the Running receipt and
                    // receipt persistence is an UPSERT — writing None here
                    // would erase established identity. Preserve it.
                    // If even that write fails, nothing was dispatched, so
                    // fail closed with the persistence fault made explicit
                    // alongside the cause.
                    let physical_outcome = format!("spawn failed: {e}");
                    if let Err(receipt_error) = this
                        .record_request_receipt_staged(
                            ReceiptStage::Terminal,
                            &dedup_id_str,
                            Some(&exec_id_str),
                            "Failed",
                        )
                        .await
                    {
                        let err = LocalIpcError::PersistenceFailure {
                            execution_id: exec_id_str,
                            stage: "terminal_receipt".to_string(),
                            physical_outcome,
                            detail: receipt_error.to_string(),
                        };
                        this.fail_task_terminal(&dedup_id_str, Err(err.clone()))
                            .await;
                        return Err(err);
                    }
                    let err = LocalIpcError::InternalRuntimeError(format!("Execution failed: {e}"));
                    let mut in_flight = this.in_flight_executions.lock().await;
                    if let Some(tx) = in_flight.remove(&dedup_id_str) {
                        let _ = tx.send(Err(err.clone()));
                    }
                    Err(err)
                }
            }
        });

        match join_handle.await {
            Ok(res) => res,
            Err(e) => {
                // Failure must return control: a panicking broker task must
                // not strand coalesced in-flight subscribers (or future
                // cancel observers) on a broadcast that will never send.
                // The receipt stays non-terminal (Running/Cancellation-
                // Requested) so recovery reconciles it to Unknown honestly.
                let err =
                    LocalIpcError::InternalRuntimeError(format!("Execution task panicked: {e}"));
                let mut in_flight = self.in_flight_executions.lock().await;
                if let Some(tx) = in_flight.remove(dedup_id) {
                    let _ = tx.send(Err(err.clone()));
                }
                Err(err)
            }
        }
    }
}
