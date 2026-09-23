use crate::codex::{
    CODEX_PROVIDER_ID, CodexAdapter, CodexRouteConfig, codex_descriptor, resolve_codex_exe,
};
use crate::conformance::ProviderCapability;
use crate::diagnostic_provider::DiagnosticAgentProvider;
use crate::openai_responses::{
    OpenAiResponsesConfig, OpenAiResponsesProvider, openai_api_key_from_env,
    openai_credential_source,
};
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

/// User-facing Luna preset identity (registered plug id; not the transport name).
pub const OPENAI_LUNA_PROVIDER_ID: &str = "openai-luna";
/// Current Omen OpenAI Platform project policy preset model.
pub const OPENAI_LUNA_MODEL: &str = "gpt-6-luna";

const OPENAI_LUNA_ID: &str = OPENAI_LUNA_PROVIDER_ID;

/// Luna preset configuration over the generic OpenAI Responses transport.
pub fn openai_luna_config() -> OpenAiResponsesConfig {
    OpenAiResponsesConfig::new(OPENAI_LUNA_MODEL)
}

/// Luna preset transport wired with local credential presence only.
///
/// This constructs the reusable transport; `ProviderDescriptor` ownership
/// remains at the registry layer via [`openai_luna_descriptor`].
pub fn openai_luna_provider() -> OpenAiResponsesProvider {
    OpenAiResponsesProvider::with_config(
        openai_luna_config(),
        openai_api_key_from_env(),
        OPENAI_LUNA_PROVIDER_ID.to_string(),
    )
}

/// Builds the Luna preset registry descriptor without exposing credentials.
pub fn openai_luna_descriptor(model: impl Into<String>, available: bool) -> ProviderDescriptor {
    ProviderDescriptor {
        id: OPENAI_LUNA_PROVIDER_ID.into(),
        name: "OpenAI GPT-6 Luna".into(),
        model: Some(model.into()),
        credential_source: Some(openai_credential_source()),
        // Canonical capability vocabulary only (see conformance module).
        // Descriptor JSON is unchanged: plain strings, no branded spellings.
        capabilities: vec![
            ProviderCapability::Reasoning.as_str().into(),
            ProviderCapability::StructuredResponse.as_str().into(),
            ProviderCapability::FailureDiagnosis.as_str().into(),
            ProviderCapability::Navigation.as_str().into(),
            ProviderCapability::Proposal.as_str().into(),
            ProviderCapability::ToolProposal.as_str().into(),
        ],
        // Locally configured enough to attempt use; remote entitlement is not claimed.
        is_available: available,
    }
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
                ProviderCapability::Orientation.as_str().into(),
                ProviderCapability::FailureDiagnosis.as_str().into(),
                ProviderCapability::Navigation.as_str().into(),
                ProviderCapability::BuildCheck.as_str().into(),
                ProviderCapability::Deterministic.as_str().into(),
            ],
            is_available: true,
        };
        let diag_provider: Arc<dyn AgentProvider> = Arc::new(TimeoutProvider::new(
            Arc::new(DiagnosticAgentProvider::new()),
            DEFAULT_AGENT_TIMEOUT,
        ));
        map.insert("diagnostic".into(), (diag_desc, diag_provider));

        // Register the Luna preset whenever a credential is locally configured.
        // Credential presence means "configured enough to attempt use"; it does
        // not prove remote key validity, model entitlement, or network reach.
        // Runtime HTTP truth remains authoritative for those outcomes.
        // The key itself is never stored on the descriptor.
        if openai_api_key_from_env().is_some() {
            let luna = openai_luna_provider();
            let desc = openai_luna_descriptor(luna.model(), true);
            let luna_provider: Arc<dyn AgentProvider> =
                Arc::new(TimeoutProvider::new(Arc::new(luna), DEFAULT_AGENT_TIMEOUT));
            map.insert(OPENAI_LUNA_ID.into(), (desc, luna_provider));
        }

        // Register the Codex external reference route whenever its binary is
        // resolvable on PATH. Resolution is filesystem-only (no spawn, no
        // network, no credits): availability means "installed enough to
        // attempt use". Account validity remains runtime truth discovered at
        // use time (AuthenticationRequired), never claimed at startup.
        if let Some(codex_exe) = resolve_codex_exe() {
            let parent_env: Vec<(String, String)> = std::env::vars().collect();
            let codex = CodexAdapter::new(CodexRouteConfig::new(codex_exe, parent_env));
            let desc = codex_descriptor(true);
            let codex_provider: Arc<dyn AgentProvider> = Arc::new(TimeoutProvider::new(
                Arc::new(codex),
                crate::codex::CODEX_ROUTE_TIMEOUT,
            ));
            map.insert(CODEX_PROVIDER_ID.into(), (desc, codex_provider));
        }

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
