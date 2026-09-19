use crate::diagnostic_provider::DiagnosticAgentProvider;
use crate::provider::{AgentError, AgentProvider, DEFAULT_AGENT_TIMEOUT, TimeoutProvider};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Metadata descriptor for an installed Agent reasoning provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub credential_source: Option<String>,
    pub capabilities: Vec<String>,
    pub is_available: bool,
}

type ProviderEntry = (ProviderDescriptor, Arc<dyn AgentProvider>);

/// Registry managing installed Agent providers and active selection.
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, ProviderEntry>>,
    active_id: RwLock<String>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    /// Creates a new ProviderRegistry with the built-in DiagnosticAgentProvider active.
    pub fn new() -> Self {
        let mut map = HashMap::new();
        let diag_desc = ProviderDescriptor {
            id: "diagnostic".into(),
            name: "Omen Built-in Diagnostic Agent".into(),
            model: Some("deterministic".into()),
            credential_source: Some("none".into()),
            capabilities: vec![
                "orientation".into(),
                "failure-diagnosis".into(),
                "navigation".into(),
                "build-check".into(),
                "deterministic".into(),
            ],
            is_available: true,
        };
        let diag_provider: Arc<dyn AgentProvider> = Arc::new(TimeoutProvider::new(
            Arc::new(DiagnosticAgentProvider::new()),
            DEFAULT_AGENT_TIMEOUT,
        ));
        map.insert("diagnostic".into(), (diag_desc, diag_provider));

        Self {
            providers: RwLock::new(map),
            active_id: RwLock::new("diagnostic".into()),
        }
    }

    /// Registers a new provider into the registry.
    pub fn register(&self, descriptor: ProviderDescriptor, provider: Arc<dyn AgentProvider>) {
        let id = descriptor.id.clone();
        let mut map = self.providers.write().unwrap();
        map.insert(id, (descriptor, provider));
    }

    /// Selects the active provider by ID.
    pub fn set_active_provider(&self, id: &str) -> Result<(), AgentError> {
        let map = self.providers.read().unwrap();
        if !map.contains_key(id) {
            return Err(AgentError::ProviderUnavailable {
                provider: id.to_string(),
                message: format!("Provider '{id}' is not installed in the Omen registry."),
            });
        }
        let mut active = self.active_id.write().unwrap();
        *active = id.to_string();
        Ok(())
    }

    /// Returns the active AgentProvider instance.
    pub fn active_provider(&self) -> Arc<dyn AgentProvider> {
        let active_id = self.active_id.read().unwrap().clone();
        let map = self.providers.read().unwrap();
        map.get(&active_id)
            .map(|(_, p)| Arc::clone(p))
            .unwrap_or_else(|| {
                // Fallback to diagnostic if somehow active_id was removed
                map.get("diagnostic")
                    .map(|(_, p)| Arc::clone(p))
                    .expect("Default diagnostic provider must exist")
            })
    }

    /// Returns the descriptor of the currently active provider.
    pub fn active_descriptor(&self) -> ProviderDescriptor {
        let active_id = self.active_id.read().unwrap().clone();
        let map = self.providers.read().unwrap();
        map.get(&active_id)
            .map(|(d, _)| d.clone())
            .unwrap_or_else(|| {
                map.get("diagnostic")
                    .map(|(d, _)| d.clone())
                    .expect("Default diagnostic provider must exist")
            })
    }

    /// Lists all registered providers.
    pub fn list_providers(&self) -> Vec<ProviderDescriptor> {
        let map = self.providers.read().unwrap();
        let mut list: Vec<_> = map.values().map(|(d, _)| d.clone()).collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// Formats structured provider status without exposing internal plumbing or credentials.
    pub fn status_text(&self) -> String {
        let active = self.active_descriptor();
        let active_mark = if active.is_available {
            "available"
        } else {
            "unavailable"
        };
        let model = active.model.as_deref().unwrap_or("default");
        let auth = active.credential_source.as_deref().unwrap_or("none");
        let caps = active.capabilities.join(", ");

        format!(
            "Provider: {} ({})\nModel: {}\nAuth source: {}\nCapabilities: {}\nStatus: {}",
            active.id, active.name, model, auth, caps, active_mark
        )
    }
}
