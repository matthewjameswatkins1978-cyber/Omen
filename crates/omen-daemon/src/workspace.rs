use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use omen_ipc::{EventPayload, FactInfo, IpcEvent, ManagedServiceInfo, SharedIndexSnapshot};
use omen_knowledge::{
    Database, FactRegistry, RequestReceiptRecord, ServiceRecord, WorkspacePersistence,
    deterministic_workspace_id, resolve_workspace_dir,
};

pub struct WorkspaceState {
    workspace_id: String,
    canonical_path: PathBuf,
    epoch: u64,
    sequence: Arc<AtomicU64>,
    facts: RwLock<HashMap<String, FactInfo>>,
    services: RwLock<HashMap<String, ManagedServiceInfo>>,
    db: Arc<Mutex<Database>>,
    event_tx: broadcast::Sender<IpcEvent>,
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
}

impl WorkspaceState {
    pub fn new(canonical_path: PathBuf, epoch: u64) -> Self {
        let workspace_id = deterministic_workspace_id(&canonical_path);
        let state_dir = resolve_workspace_dir(&canonical_path);
        let db = Database::open(&state_dir.join("knowledge.db"))
            .unwrap_or_else(|_| Database::open_in_memory().expect("In-memory database fallback"));

        let _ = WorkspacePersistence::upsert_workspace(
            &db,
            &workspace_id,
            &canonical_path.to_string_lossy(),
            epoch,
        );

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
                initial_services.insert(
                    s.name.clone(),
                    ManagedServiceInfo {
                        name: s.name.clone(),
                        resource_uri: format!("proc://workspace/{}", s.name),
                        pid: s.pid,
                        command: s.command,
                        state: s.state,
                        uptime_secs: 0,
                    },
                );
            }
        }

        let (event_tx, _) = broadcast::channel(512);

        Self {
            workspace_id,
            canonical_path,
            epoch,
            sequence: Arc::new(AtomicU64::new(1)),
            facts: RwLock::new(initial_facts),
            services: RwLock::new(initial_services),
            db: Arc::new(Mutex::new(db)),
            event_tx,
            watcher: Arc::new(Mutex::new(None)),
        }
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
                    let has_relevant_change =
                        event.paths.iter().any(|p| !Self::is_ignored_path(p));
                    if has_relevant_change {
                        if let Some(ws) = this.upgrade() {
                            let cause = format!("fs:mutation:{:?}", event.kind);
                            rt_clone.spawn(async move {
                                ws.invalidate_all_current_facts(&cause).await;
                            });
                        }
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
        services.values().cloned().collect()
    }

    pub async fn get_service(&self, name: &str) -> Option<ManagedServiceInfo> {
        let services = self.services.read().await;
        services.get(name).cloned()
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
}
