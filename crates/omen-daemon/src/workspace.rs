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
    Database, ExecutionHistory, ExecutionRecord, FactRegistry, RequestReceiptRecord, ServiceRecord,
    WorkspacePersistence, canonical_workspace_db_path, cas::ContentAddressedStore,
    deterministic_workspace_id, resolve_workspace_dir,
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
fn recorded_history_runtime(rec: &ExecutionRecord) -> Option<omen_core::RuntimeStatus> {
    let envelope = rec.envelope_json.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(envelope).ok()?;
    match value.get("status")?.as_str()? {
        "COMPLETED" | "FAILED" | "SPAWN_FAILED" | "CONTAINMENT_FAILED" | "IO_FAILED" => {
            Some(omen_core::RuntimeStatus::Completed)
        }
        "TIMED_OUT" => Some(omen_core::RuntimeStatus::TimedOut),
        "CANCELLED" => Some(omen_core::RuntimeStatus::Cancelled),
        "OUTCOME_UNKNOWN" => Some(omen_core::RuntimeStatus::OutcomeUnknown),
        _ => None,
    }
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
    event_tx: broadcast::Sender<IpcEvent>,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
}

impl WorkspaceState {
    pub fn new(canonical_path: PathBuf, epoch: u64) -> Result<Self, LocalIpcError> {
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
        let supervisor = Arc::new(ProcessSupervisor::new());
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
            event_tx,
            watcher: Arc::new(Mutex::new(None)),
        })
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

    pub async fn record_request_receipt(&self, req_id: &str, exec_id: Option<&str>, status: &str) {
        let db = self.db.lock().await;
        let _ = WorkspacePersistence::record_request_receipt(
            &db,
            &RequestReceiptRecord {
                consequential_request_id: req_id.to_string(),
                execution_id: exec_id.map(Into::into),
                status: status.to_string(),
                recorded_at: chrono::Utc::now().to_rfc3339(),
            },
        );
    }

    pub async fn query_request_receipt(&self, req_id: &str) -> Option<RequestReceiptRecord> {
        let db = self.db.lock().await;
        WorkspacePersistence::get_request_receipt(&db, req_id)
            .ok()
            .flatten()
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
                self.record_request_receipt(
                    &rec.consequential_request_id,
                    Some(execution_id),
                    "Unknown",
                )
                .await;
                mk(
                    CancelOutcome::OutcomeUnknown,
                    "no live execution task; receipt reconciled to Unknown",
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
            self.record_request_receipt(&dedup_id, Some(execution_id), "CancellationRequested")
                .await;
            let _ = cancel_tx.send(true);
            if let Some(sub) = self.subscribe_cancel_terminal(&dedup_id).await
                && let Some(report) = self.await_cancel_terminal(execution_id, sub).await
            {
                return report;
            }
            // Unobservable broadcast: fall through to the durable receipt.
        }

        // Durable receipt decides. A task that is still alive holds its
        // in_flight entry until after its terminal receipt write, so prefer
        // live observation for non-terminal receipts; otherwise report.
        let receipt = {
            let db = self.db.lock().await;
            WorkspacePersistence::get_receipt_by_execution_id(&db, execution_id)
                .ok()
                .flatten()
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
                    let reread = {
                        let db = self.db.lock().await;
                        WorkspacePersistence::get_receipt_by_execution_id(&db, execution_id)
                            .ok()
                            .flatten()
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
        let envelope_json = runtime.map(|status| {
            let history_status = status.history_label(exit_code);
            format!(r#"{{"status":"{history_status}","source":"broker"}}"#)
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

        // 2. Check if already recorded in request_receipts in SQLite
        if let Some(receipt) = self.query_request_receipt(dedup_id).await {
            if receipt.status == "Unknown" {
                return Err(LocalIpcError::ExecutionStatusUnknown(dedup_id.to_string()));
            }
            if receipt.status == "Completed"
                && let Some(exec_id) = receipt.execution_id
            {
                let db = self.db.lock().await;
                if let Ok(eid) = ExecutionId::new(&exec_id)
                    && let Ok(Some(rec)) = ExecutionHistory::get_execution(&db, &eid)
                {
                    // E2 replay truth: reproduce the recorded terminal
                    // outcome, not a hardcoded Completed. The broker stamps
                    // every record with an explicit history status envelope;
                    // legacy records without one keep the previous fallback
                    // (exit present → Completed, absent → TimedOut, the only
                    // exit-None case the Completed receipt ever covered).
                    let runtime_status =
                        recorded_history_runtime(&rec).unwrap_or(match rec.exit_code {
                            Some(_) => omen_core::RuntimeStatus::Completed,
                            None => omen_core::RuntimeStatus::TimedOut,
                        });
                    let summary = ExecutionResultSummary {
                        execution_id: exec_id,
                        runtime_status,
                        exit_code: rec.exit_code,
                        duration_ms: rec.duration_ms.unwrap_or(0) as u64,
                        stdout_preview: rec.command.clone(),
                        stderr_preview: String::new(),
                        stdout_artifact: rec.stdout_artifact,
                        stderr_artifact: rec.stderr_artifact,
                    };
                    self.execution_cache
                        .write()
                        .await
                        .insert(dedup_id.to_string(), summary.clone());
                    return Ok(summary);
                }
            }
        }

        // 3. Check if currently in-flight
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

        let execution_id = ExecutionId::generate();
        let exec_id_str = execution_id.to_string();

        // 4. Mark Running in SQLite receipt with the canonical execution_id
        self.record_request_receipt(dedup_id, Some(&exec_id_str), "Running")
            .await;

        // 5. Construct argv
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
            let mut in_flight = self.in_flight_executions.lock().await;
            in_flight.remove(dedup_id);
            self.record_request_receipt(dedup_id, Some(&exec_id_str), "Failed")
                .await;
            return Err(LocalIpcError::MalformedRequest(
                "Command argv cannot be empty".into(),
            ));
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

                    // Record history with the same canonical execution_id
                    let _ = this
                        .record_history_with_id(
                            Some(execution_id),
                            &session_id_str,
                            &cmd_str,
                            exit_code,
                            duration_ms,
                            stdout_art.clone(),
                            stderr_art.clone(),
                            history_runtime,
                        )
                        .await;

                    // Update request receipt
                    this.record_request_receipt(
                        &dedup_id_str,
                        Some(&exec_id_str),
                        terminal_receipt,
                    )
                    .await;

                    let summary = ExecutionResultSummary {
                        execution_id: exec_id_str,
                        runtime_status: output.runtime_status,
                        exit_code,
                        duration_ms,
                        stdout_preview,
                        stderr_preview,
                        stdout_artifact: stdout_art,
                        stderr_artifact: stderr_art,
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
                    this.record_request_receipt(&dedup_id_str, None, "Failed")
                        .await;
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
