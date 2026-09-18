use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::workspace::WorkspaceState;

pub struct WorkspaceRegistry {
    epoch: u64,
    by_id: RwLock<HashMap<String, Arc<WorkspaceState>>>,
    by_path: RwLock<HashMap<PathBuf, Arc<WorkspaceState>>>,
}

impl WorkspaceRegistry {
    pub fn new(epoch: u64) -> Self {
        Self {
            epoch,
            by_id: RwLock::new(HashMap::new()),
            by_path: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get_or_attach(&self, path: &Path) -> Arc<WorkspaceState> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        {
            let by_path = self.by_path.read().await;
            if let Some(ws) = by_path.get(&canonical) {
                return ws.clone();
            }
        }

        let mut by_path = self.by_path.write().await;
        let mut by_id = self.by_id.write().await;

        if let Some(ws) = by_path.get(&canonical) {
            return ws.clone();
        }

        let state = Arc::new(WorkspaceState::new(canonical.clone(), self.epoch));
        by_id.insert(state.workspace_id().to_string(), state.clone());
        by_path.insert(canonical, state.clone());
        state
    }

    pub async fn get_by_id(&self, id: &str) -> Option<Arc<WorkspaceState>> {
        let by_id = self.by_id.read().await;
        by_id.get(id).cloned()
    }

    pub async fn list_workspaces(&self) -> Vec<Arc<WorkspaceState>> {
        let by_id = self.by_id.read().await;
        by_id.values().cloned().collect()
    }
}
