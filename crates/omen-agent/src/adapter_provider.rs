//! Generic NDJSON adapter as an [`AgentProvider`].
//!
//! One launch per request, no retries, no fallback substitution. Tool
//! requests are validated against an explicit handler allowlist and
//! executed through Omen-side functions; proposed actions are validated
//! with the G1 [`validate_proposed_action`] gate before admission.

use crate::adapter_spawn::{AdapterSpawnError, SpawnLimits, SpawnedAdapter};
use crate::conformance::validate_proposed_action;
use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind, ProposedAction,
};
use omen_agent_adapter::negotiate::{AdapterOffer, Negotiated, OmenOffer, negotiate};
use omen_agent_adapter::protocol::{
    ADAPTER_PROTOCOL_VERSION, AdapterErrorCode, AdapterFrame, OMEN_SUPPORTED_ADAPTER_PROTOCOLS,
    OmenFrame, OmenHello, Orientation, ReasonRequest, ToolResult,
};
use omen_agent_adapter::{
    AdapterManifest, BindingIdentity, EnvPolicy, fingerprint_pairs, sha256_hex,
};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Omen-side tool handler: pure function of bounded JSON input.
pub type AdapterToolHandler =
    Arc<dyn Fn(&serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>;

/// Fixed allowlist of Omen operations a generic adapter may request.
/// Anything else is refused, never reinterpreted.
pub const GENERIC_ADAPTER_TOOL_ALLOWLIST: &[&str] = &["omen.describe", "omen.workspace_status"];

/// `omen.describe` static capability catalogue (progressive disclosure seed).
pub fn describe_tool_handler(input: &serde_json::Value) -> Result<serde_json::Value, String> {
    let target = input
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing string field 'target'".to_string())?;
    if target.len() > 256 {
        return Err("target too long".into());
    }
    let description = match target {
        "fixture" => "Deterministic test fixture: answers from machine truth, never the network.",
        "reasoning" => {
            "Reasoning route: an external model translating prompts into typed Omen responses."
        }
        "tools" => {
            "Tool round trip: adapter requests an allowlisted Omen operation; Omen executes it under admission and returns a typed result."
        }
        _ => return Err(format!("unknown describe target {target:?}")),
    };
    Ok(serde_json::json!({"target": target, "description": description}))
}

/// `omen.workspace_status` deterministic snapshot (no model, no network).
pub fn workspace_status_tool_handler(
    root: PathBuf,
    cwd: PathBuf,
    version: String,
    contract: String,
) -> AdapterToolHandler {
    Arc::new(move |input: &serde_json::Value| {
        if !input.is_null() && !input.as_object().map(|o| o.is_empty()).unwrap_or(false) {
            return Err("omen.workspace_status takes no input".into());
        }
        Ok(serde_json::json!({
            "workspace_root": root.to_string_lossy(),
            "cwd": cwd.to_string_lossy(),
            "omen_version": version,
            "omen_contract": contract,
        }))
    })
}

#[derive(Clone)]
pub struct AdapterProviderConfig {
    pub manifest: AdapterManifest,
    pub exe: PathBuf,
    pub extra_argv: Vec<String>,
    pub env_policy: EnvPolicy,
    pub parent_env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub omen_contract: String,
    pub omen_version: String,
    pub limits: SpawnLimits,
    pub tool_handlers: Vec<(String, AdapterToolHandler)>,
}

pub struct AdapterBackedProvider {
    config: AdapterProviderConfig,
    exe_digest: String,
    manifest_digest: String,
}

impl AdapterBackedProvider {
    pub fn new(config: AdapterProviderConfig) -> Result<Self, AgentError> {
        config
            .manifest
            .validate()
            .map_err(|e| AgentError::Rejected(format!("invalid adapter manifest: {e}")))?;
        let exe_bytes =
            std::fs::read(&config.exe).map_err(|e| AgentError::ProviderUnavailable {
                provider: config.manifest.adapter_id.clone(),
                message: format!("adapter executable unreadable: {e}"),
            })?;
        let manifest_bytes = serde_json::to_vec(&config.manifest)
            .map_err(|e| AgentError::Rejected(format!("manifest not serializable: {e}")))?;
        Ok(Self {
            exe_digest: sha256_hex(&exe_bytes),
            manifest_digest: sha256_hex(&manifest_bytes),
            config,
        })
    }

    pub fn adapter_id(&self) -> &str {
        &self.config.manifest.adapter_id
    }

    pub fn binding_identity(&self, negotiated: &Negotiated) -> BindingIdentity {
        let pairs = [
            (
                "argv0",
                self.config.exe.to_string_lossy().as_ref().to_string(),
            ),
            ("extra_argv", self.config.extra_argv.join("\u{1f}")),
            ("transport", "stdio-ndjson".to_string()),
            ("protocol", negotiated.protocol_version.clone()),
        ];
        let refs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        BindingIdentity {
            adapter_id: self.config.manifest.adapter_id.clone(),
            adapter_version: self.config.manifest.adapter_version.clone(),
            adapter_digest: self.exe_digest.clone(),
            manifest_digest: self.manifest_digest.clone(),
            protocol_version: negotiated.protocol_version.clone(),
            provider_id: self.config.manifest.adapter_id.clone(),
            model: None,
            transport: "stdio-ndjson".into(),
            omen_contract: self.config.omen_contract.clone(),
            config_fingerprint: fingerprint_pairs(&refs),
            bound_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    async fn exchange(&self, request: AgentRequest) -> Result<AgentResponse, AgentError> {
        let env = self.config.env_policy.build_env(
            &self.config.parent_env,
            &self.config.manifest.credential_labels,
        );
        let total = self.config.limits.total_timeout;
        match tokio::time::timeout(total, self.exchange_inner(request, env)).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(total)),
        }
    }

    async fn exchange_inner(
        &self,
        request: AgentRequest,
        env: Vec<(String, String)>,
    ) -> Result<AgentResponse, AgentError> {
        let id = self.config.manifest.adapter_id.clone();
        let mut child = SpawnedAdapter::launch(
            &self.config.exe,
            &self.config.extra_argv,
            &env,
            &self.config.cwd,
            self.config.limits.clone(),
        )
        .await
        .map_err(|e| spawn_to_agent_error(&id, e))?;

        let result = self.handshake_and_reason(&mut child, &request).await;
        // The child never outlives the request: graceful shutdown, then kill.
        let exit = child.shutdown().await;
        let mut result = result;
        if let Err(AgentError::Provider(msg)) = &result
            && msg.contains("EOF")
        {
            result = Err(AgentError::Provider(format!(
                "{msg} (child exit: {:?})",
                exit.code
            )));
        }
        result
    }

    async fn handshake_and_reason(
        &self,
        child: &mut SpawnedAdapter,
        request: &AgentRequest,
    ) -> Result<AgentResponse, AgentError> {
        let id = self.config.manifest.adapter_id.clone();
        let offer = OmenOffer {
            omen_contract: self.config.omen_contract.clone(),
            adapter_protocols: OMEN_SUPPORTED_ADAPTER_PROTOCOLS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            required_features: self.config.manifest.required_features.clone(),
            optional_features: self.config.manifest.optional_features.clone(),
        };
        child
            .send(&OmenFrame::Hello {
                id: 1,
                payload: OmenHello {
                    protocol: format!(
                        "{}/{}",
                        omen_agent_adapter::protocol::ADAPTER_PROTOCOL_SCHEMA,
                        ADAPTER_PROTOCOL_VERSION
                    ),
                    omen_contract: offer.omen_contract.clone(),
                    adapter_protocols: offer.adapter_protocols.clone(),
                    required_features: offer.required_features.clone(),
                    optional_features: offer.optional_features.clone(),
                },
            })
            .await
            .map_err(|e| spawn_to_agent_error(&id, e))?;

        let hello = match child.next_frame().await {
            Ok(AdapterFrame::Hello { payload, .. }) => payload,
            Ok(AdapterFrame::Error { payload, .. }) => {
                return Err(adapter_code_to_error(&id, &payload.code, &payload.message));
            }
            Ok(other) => {
                return Err(AgentError::Rejected(format!(
                    "adapter '{id}': expected hello first, got {}",
                    frame_kind(&other)
                )));
            }
            Err(e) => return Err(spawn_to_agent_error(&id, e)),
        };
        let negotiated = negotiate(
            &offer,
            &AdapterOffer {
                protocol_version: hello.protocol_version.clone(),
                features: hello.features.clone(),
                contract_versions: hello.contract_versions.clone(),
            },
        )
        .map_err(|r| {
            AgentError::Rejected(format!(
                "adapter '{id}': negotiation refused ({:?}): {}",
                r.reason, r.message
            ))
        })?;
        let _ = negotiated;

        let handlers: HashMap<String, AdapterToolHandler> =
            self.config.tool_handlers.iter().cloned().collect();
        let allowlist: Vec<String> = handlers.keys().cloned().collect();
        let request_id = format!("req-{}", self.config.manifest.adapter_id);
        child
            .send(&OmenFrame::Reason {
                id: 2,
                payload: ReasonRequest {
                    request_id: request_id.clone(),
                    prompt: truncate_text(&request.prompt, 16 * 1024),
                    orientation: Orientation {
                        omen_contract: self.config.omen_contract.clone(),
                        adapter_protocol: ADAPTER_PROTOCOL_VERSION.to_string(),
                        capabilities: self.config.manifest.capabilities.clone(),
                        workspace_root: request
                            .context
                            .workspace_root
                            .to_string_lossy()
                            .into_owned(),
                        cwd: request.context.cwd.to_string_lossy().into_owned(),
                    },
                    tool_allowlist: allowlist,
                },
            })
            .await
            .map_err(|e| spawn_to_agent_error(&id, e))?;

        let mut next_send_id: u64 = 3;
        loop {
            match child.next_frame().await {
                Ok(AdapterFrame::Response { payload, .. }) => {
                    if payload.request_id != request_id {
                        return Err(AgentError::Rejected(format!(
                            "adapter '{id}': response for unknown request {:?}",
                            payload.request_id
                        )));
                    }
                    return self.admit_response(&id, &payload);
                }
                Ok(AdapterFrame::ToolRequest { payload, .. }) => {
                    if payload.request_id != request_id {
                        return Err(AgentError::Rejected(format!(
                            "adapter '{id}': tool request for unknown request {:?}",
                            payload.request_id
                        )));
                    }
                    let outcome = match handlers.get(&payload.tool) {
                        Some(handler) => match handler(&payload.input) {
                            Ok(value) => (true, value),
                            Err(message) => (
                                false,
                                serde_json::json!({"code":"tool_failed","message":message}),
                            ),
                        },
                        None => (
                            false,
                            serde_json::json!({"code":"unsupported_capability","message":format!("tool {:?} is not allowlisted", payload.tool)}),
                        ),
                    };
                    child
                        .send(&OmenFrame::ToolResult {
                            id: next_send_id,
                            payload: ToolResult {
                                request_id: payload.request_id,
                                call_id: payload.call_id,
                                ok: outcome.0,
                                result: outcome.1,
                            },
                        })
                        .await
                        .map_err(|e| spawn_to_agent_error(&id, e))?;
                    next_send_id += 1;
                }
                Ok(AdapterFrame::Error { payload, .. }) => {
                    return Err(adapter_code_to_error(&id, &payload.code, &payload.message));
                }
                Ok(AdapterFrame::Hello { .. }) => {
                    return Err(AgentError::Rejected(format!(
                        "adapter '{id}': duplicate hello mid-request"
                    )));
                }
                Err(e) => return Err(spawn_to_agent_error(&id, e)),
            }
        }
    }

    fn admit_response(
        &self,
        id: &str,
        payload: &omen_agent_adapter::protocol::AdapterResponse,
    ) -> Result<AgentResponse, AgentError> {
        let kind = match payload.kind.as_str() {
            "explanation" => AgentResponseKind::Explanation,
            "proposal" => AgentResponseKind::Proposal,
            "action_request" => AgentResponseKind::ActionRequest,
            "result" => AgentResponseKind::Result,
            "question" => AgentResponseKind::Question,
            "refusal" => AgentResponseKind::Refusal,
            other => {
                return Err(AgentError::Rejected(format!(
                    "adapter '{id}': unknown response kind {other:?}"
                )));
            }
        };
        let mut actions = Vec::new();
        for value in &payload.proposed_actions {
            let action: ProposedAction = serde_json::from_value(value.clone()).map_err(|e| {
                AgentError::Rejected(format!("adapter '{id}': malformed proposed action: {e}"))
            })?;
            validate_proposed_action(&action).map_err(|e| {
                AgentError::Rejected(format!("adapter '{id}': invalid proposed action: {e}"))
            })?;
            actions.push(action);
        }
        Ok(AgentResponse {
            kind,
            message: truncate_text(&payload.message, 16 * 1024),
            proposed_actions: actions,
            references: payload.references.iter().take(16).cloned().collect(),
            uncertainty: payload.uncertainty.clone(),
        })
    }
}

impl AgentProvider for AdapterBackedProvider {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(self.exchange(request))
    }
}

fn frame_kind(frame: &AdapterFrame) -> &'static str {
    match frame {
        AdapterFrame::Hello { .. } => "hello",
        AdapterFrame::Response { .. } => "response",
        AdapterFrame::ToolRequest { .. } => "tool_request",
        AdapterFrame::Error { .. } => "error",
    }
}

pub fn adapter_code_to_error(id: &str, code: &str, message: &str) -> AgentError {
    let (parsed, known) = AdapterErrorCode::parse(code);
    let bounded = truncate_text(message, 1024);
    // Unknown vendor codes collapse to Provider with bounded detail: they
    // never widen Omen semantics.
    let _ = known;
    match parsed {
        AdapterErrorCode::AuthRequired => AgentError::AuthenticationRequired {
            provider: id.into(),
            message: bounded,
        },
        AdapterErrorCode::Unavailable => AgentError::ProviderUnavailable {
            provider: id.into(),
            message: bounded,
        },
        AdapterErrorCode::RateLimited => AgentError::RateLimited {
            provider: id.into(),
            retry_after_secs: None,
        },
        AdapterErrorCode::Timeout => AgentError::Timeout(Duration::from_secs(30)),
        AdapterErrorCode::UnsupportedCapability => AgentError::UnsupportedCapability {
            provider: id.into(),
            capability: bounded,
        },
        AdapterErrorCode::Malformed
        | AdapterErrorCode::Incompatible
        | AdapterErrorCode::Internal
        | AdapterErrorCode::TransportFailure => {
            AgentError::Provider(format!("adapter '{id}' error [{code}]: {bounded}"))
        }
    }
}

fn spawn_to_agent_error(id: &str, e: AdapterSpawnError) -> AgentError {
    match e {
        AdapterSpawnError::SpawnFailed { message, .. } => AgentError::ProviderUnavailable {
            provider: id.into(),
            message: format!("adapter spawn failed: {message}"),
        },
        AdapterSpawnError::FrameTimeout(d) => AgentError::Timeout(d),
        AdapterSpawnError::Eof => AgentError::Provider(format!("adapter '{id}' EOF")),
        AdapterSpawnError::Exited { code, stderr_tail } => AgentError::Provider(format!(
            "adapter '{id}' exited with status {code:?} (stderr: {})",
            truncate_text(&stderr_tail, 512)
        )),
        // Wire-level refusal: the adapter's bytes were not a valid response.
        // Rejected, never retried, never reinterpreted as transport failure.
        // (G1 MALFORMED_RESPONSE class.)
        AdapterSpawnError::Protocol(p) => AgentError::Rejected(format!("adapter '{id}': {p}")),
        AdapterSpawnError::Io(msg) => AgentError::Provider(format!("adapter '{id}' IO: {msg}")),
    }
}

fn truncate_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...[truncated]", &s[..max])
    }
}
