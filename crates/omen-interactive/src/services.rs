use omen_core::{CoreError, ResourceUri};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedService {
    pub name: String,
    pub resource_uri: ResourceUri,
    pub pid: Option<u32>,
    pub command: String,
    pub state: ServiceState,
    pub uptime_secs: u64,
}

pub struct ServiceRegistry {
    services: Arc<Mutex<HashMap<String, ManagedService>>>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self {
            services: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn global() -> &'static Self {
        static REGISTRY: OnceLock<ServiceRegistry> = OnceLock::new();
        REGISTRY.get_or_init(Self::new)
    }

    pub fn register(&self, service: ManagedService) {
        if let Ok(mut lock) = self.services.lock() {
            lock.insert(service.name.clone(), service);
        }
    }

    pub fn list(&self) -> Vec<ManagedService> {
        if let Ok(lock) = self.services.lock() {
            let mut items: Vec<_> = lock.values().cloned().collect();
            items.sort_by(|a, b| a.name.cmp(&b.name));
            items
        } else {
            Vec::new()
        }
    }

    pub fn get(&self, name: &str) -> Option<ManagedService> {
        if let Ok(lock) = self.services.lock() {
            lock.get(name).cloned()
        } else {
            None
        }
    }

    pub fn stop(&self, name: &str) -> Result<bool, CoreError> {
        if let Ok(mut lock) = self.services.lock() {
            if let Some(svc) = lock.get_mut(name) {
                svc.state = ServiceState::Stopped;
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Err(CoreError::Internal("Lock poisoned".into()))
        }
    }
}
