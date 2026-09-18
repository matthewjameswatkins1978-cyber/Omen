use sha2::Digest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast};

use omen_ipc::{EventPayload, FactInfo, IpcEvent, ManagedServiceInfo, SharedIndexSnapshot};

pub struct WorkspaceState {
    workspace_id: String,
    canonical_path: PathBuf,
    epoch: u64,
    sequence: Arc<AtomicU64>,
    facts: RwLock<HashMap<String, FactInfo>>,
    services: RwLock<HashMap<String, ManagedServiceInfo>>,
    event_tx: broadcast::Sender<IpcEvent>,
}

impl WorkspaceState {
    pub fn new(canonical_path: PathBuf, epoch: u64) -> Self {
        let path_str = canonical_path.to_string_lossy().to_string();
        let digest = hex::encode(sha2::Sha256::digest(path_str.as_bytes()));
        let workspace_id = format!("ws_{}", &digest[..16]);
        let (event_tx, _) = broadcast::channel(512);

        Self {
            workspace_id,
            canonical_path,
            epoch,
            sequence: Arc::new(AtomicU64::new(1)),
            facts: RwLock::new(HashMap::new()),
            services: RwLock::new(HashMap::new()),
            event_tx,
        }
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

        {
            let mut services = self.services.write().await;
            services.insert(name.clone(), info);
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

            drop(services);

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
}
