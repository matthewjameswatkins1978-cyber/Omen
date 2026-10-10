//! Anthropic Claude Sonnet adapter over the Messages API (mocked-first).
//!
//! Model ID `claude-sonnet-5-5` verified against the Claude Platform docs
//! (Models overview, extracted 2026-10-10; API alias, default effort high,
//! 1M context, 128K max output). Error taxonomy verified against the API
//! errors reference (401 authentication, 429 `rate_limit_error`, 500).
//!
//! Inactive by default: the registry only lists this provider when
//! `ANTHROPIC_API_KEY` is present, and it never becomes the default.
//! Activation is explicit (`:agent use anthropic-sonnet`). No live calls
//! happen in tests: all behavior below is proven through the shared
//! [`HttpPost`] mock seam.
//!
//! Structured output is instruction, not trust (same doctrine as the Codex
//! route): the system prompt demands the Omen agent JSON shape, and the
//! adapter validates whatever text comes back before admitting it.

use crate::conformance::ProviderCapability;
use crate::openai_responses::{HttpPost, HttpResponseSnapshot, WireAgentResponse};
use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, ProviderIdentity, ProviderUsage,
};
use crate::registry::ProviderDescriptor;
use serde::Deserialize;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::provider::DEFAULT_AGENT_TIMEOUT;

/// Anthropic Messages API endpoint.
pub const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// API version header pinned for the adapter.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Credential environment variable (presence-gated like the Luna preset).
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// Verified Sonnet model ID (Claude API alias).
pub const ANTHROPIC_SONNET_MODEL: &str = "claude-sonnet-5-5";
/// Registry plug id (not the transport name).
pub const ANTHROPIC_SONNET_PROVIDER_ID: &str = "anthropic-sonnet";
/// Default output budget: same bounded-structured rationale as the Luna
/// preset (small schema, cut responses fail closed, never trail off).
pub const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 800;

pub fn anthropic_credential_source() -> String {
    format!("environment:{ANTHROPIC_API_KEY_ENV}")
}

/// Reads the API key from the process environment when present and
/// non-empty. Presence only proves configured-enough-to-attempt-use.
pub fn anthropic_api_key_from_env() -> Option<String> {
    match std::env::var(ANTHROPIC_API_KEY_ENV) {
        Ok(k) if !k.trim().is_empty() => Some(k),
        _ => None,
    }
}

/// Model-neutral configuration for the Anthropic Messages transport.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub model: String,
    pub messages_url: String,
    pub api_version: String,
    pub timeout: Duration,
    pub max_tokens: u32,
}

impl AnthropicConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages_url: ANTHROPIC_MESSAGES_URL.to_string(),
            api_version: ANTHROPIC_VERSION.to_string(),
            timeout: DEFAULT_AGENT_TIMEOUT,
            max_tokens: ANTHROPIC_DEFAULT_MAX_TOKENS,
        }
    }
}

/// Errors classified for tests and mapping into AgentError.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnthropicFailureClass {
    MissingCredential,
    AuthFailure,
    AccountNotEntitled,
    RateLimited,
    EndpointUnsupported,
    ModelNotFound,
    ProviderFailure,
    Timeout,
    MalformedResponse,
    UnsupportedFeature,
    IncompleteResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicFailure {
    pub class: AnthropicFailureClass,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl AnthropicFailure {
    pub fn to_agent_error(&self, provider: &str) -> AgentError {
        match self.class {
            AnthropicFailureClass::MissingCredential | AnthropicFailureClass::AuthFailure => {
                AgentError::AuthenticationRequired {
                    provider: provider.to_string(),
                    message: self.message.clone(),
                }
            }
            AnthropicFailureClass::AccountNotEntitled
            | AnthropicFailureClass::ModelNotFound
            | AnthropicFailureClass::EndpointUnsupported => AgentError::ProviderUnavailable {
                provider: provider.to_string(),
                message: self.message.clone(),
            },
            AnthropicFailureClass::RateLimited => AgentError::RateLimited {
                provider: provider.to_string(),
                retry_after_secs: self.retry_after_secs,
            },
            AnthropicFailureClass::Timeout => AgentError::Timeout(
                self.retry_after_secs
                    .map(Duration::from_secs)
                    .unwrap_or(DEFAULT_AGENT_TIMEOUT),
            ),
            AnthropicFailureClass::UnsupportedFeature => AgentError::UnsupportedCapability {
                provider: provider.to_string(),
                capability: self.message.clone(),
            },
            AnthropicFailureClass::ProviderFailure => {
                AgentError::Provider(format!("provider '{provider}' failure: {}", self.message))
            }
            AnthropicFailureClass::MalformedResponse => AgentError::Rejected(format!(
                "provider '{provider}' returned a malformed Messages response: {}",
                self.message
            )),
            AnthropicFailureClass::IncompleteResponse => AgentError::Rejected(format!(
                "provider '{provider}' response not completed: {}",
                self.message
            )),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MessagesEnvelope {
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    content: Vec<Value>,
    #[serde(default)]
    usage: Option<MessagesUsage>,
    #[serde(default)]
    error: Option<MessagesErrorBody>,
}

#[derive(Debug, Deserialize)]
struct MessagesUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct MessagesErrorBody {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    message: String,
}

/// Anthropic Claude Sonnet AgentProvider over the Messages API.
///
/// Single attempt per respond (no retries, never billable beyond the one
/// call the caller explicitly asked for). Secrets are redacted from every
/// surfaced error and bounded body.
#[derive(Clone)]
pub struct AnthropicSonnetProvider {
    config: AnthropicConfig,
    api_key: Option<String>,
    http: Arc<dyn HttpPost>,
    provider_id: String,
}

impl std::fmt::Debug for AnthropicSonnetProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicSonnetProvider")
            .field("provider_id", &self.provider_id)
            .field("model", &self.config.model)
            .field("messages_url", &self.config.messages_url)
            .field("timeout", &self.config.timeout)
            .field("max_tokens", &self.config.max_tokens)
            .field("credential", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("has_credential", &self.api_key.is_some())
            .finish()
    }
}

impl AnthropicSonnetProvider {
    pub fn with_config(
        config: AnthropicConfig,
        api_key: Option<String>,
        provider_id: impl Into<String>,
    ) -> Self {
        let http = crate::openai_responses::UreqHttpPost::new(config.timeout);
        Self {
            config,
            api_key,
            http: Arc::new(http),
            provider_id: provider_id.into(),
        }
    }

    pub fn with_http(
        config: AnthropicConfig,
        api_key: Option<String>,
        http: Arc<dyn HttpPost>,
        provider_id: impl Into<String>,
    ) -> Self {
        Self {
            config,
            api_key,
            http,
            provider_id: provider_id.into(),
        }
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn has_credential(&self) -> bool {
        self.api_key.as_ref().is_some_and(|k| !k.trim().is_empty())
    }

    fn build_request_body(&self, request: &AgentRequest) -> Result<Value, AgentError> {
        let context_json = serde_json::to_value(&request.context).map_err(|e| {
            AgentError::Rejected(format!("agent context serialization failed: {e}"))
        })?;
        let mut context_str = serde_json::to_string(&context_json).unwrap_or_default();
        if context_str.len() > 16_384 {
            context_str.truncate(16_384);
        }
        let mut user_text = format!(
            "Workspace context:\n{context_str}\n\nTask: {}",
            request.prompt.chars().take(4_000).collect::<String>()
        );
        for turn in request.conversation.iter().rev().take(6).rev() {
            user_text.push_str(&format!(
                "\n{}: {}",
                turn.role.chars().take(64).collect::<String>(),
                turn.text.chars().take(2_000).collect::<String>()
            ));
        }
        Ok(json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": agent_json_system_prompt(),
            "messages": [{"role": "user", "content": user_text}],
        }))
    }

    fn post(&self, body: &str) -> Result<HttpResponseSnapshot, AnthropicFailure> {
        let api_key = self.api_key.clone().ok_or(AnthropicFailure {
            class: AnthropicFailureClass::MissingCredential,
            message: format!("missing credential {ANTHROPIC_API_KEY_ENV}"),
            retry_after_secs: None,
        })?;
        let headers = vec![
            ("x-api-key".to_string(), api_key),
            (
                "anthropic-version".to_string(),
                self.config.api_version.clone(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        match self
            .http
            .post_json(&self.config.messages_url, &headers, body)
        {
            Ok(resp) => Ok(resp),
            Err(e) if e.to_lowercase().contains("timeout") => Err(AnthropicFailure {
                class: AnthropicFailureClass::Timeout,
                message: e,
                retry_after_secs: Some(self.config.timeout.as_secs()),
            }),
            Err(e) => Err(AnthropicFailure {
                class: AnthropicFailureClass::ProviderFailure,
                message: e,
                retry_after_secs: None,
            }),
        }
    }

    fn safe_error(&self, mut failure: AnthropicFailure) -> AgentError {
        if let Some(secret) = redactable_secret(self.api_key.as_deref()) {
            failure.message = failure.message.replace(secret, "[REDACTED]");
        }
        failure.to_agent_error(&self.provider_id)
    }

    /// Parses a Messages API body. Only completed stop reasons may produce
    /// an AgentResponse; `refusal` becomes a typed refusal.
    pub fn parse_messages_body(
        body: &str,
    ) -> Result<(AgentResponse, Option<ProviderUsage>), AnthropicFailure> {
        let envelope: MessagesEnvelope =
            serde_json::from_str(body).map_err(|e| AnthropicFailure {
                class: AnthropicFailureClass::MalformedResponse,
                message: format!("envelope decode failed: {e}"),
                retry_after_secs: None,
            })?;
        if let Some(error) = envelope.error {
            return Err(classify_messages_error(&error.kind, &error.message));
        }
        // Only message envelopes carry completions; anything else shaped
        // like success is malformed.
        if envelope.kind.as_deref() != Some("message") {
            return Err(AnthropicFailure {
                class: AnthropicFailureClass::MalformedResponse,
                message: format!(
                    "unexpected envelope type {:?}; refusing to guess",
                    envelope.kind
                ),
                retry_after_secs: None,
            });
        }
        match envelope.stop_reason.as_deref() {
            Some("end_turn") | Some("stop_sequence") => {}
            Some("refusal") => {
                let text = join_text_content(&envelope.content);
                let mut response = AgentResponse::refusal(if text.trim().is_empty() {
                    "Claude declined the request.".to_string()
                } else {
                    text
                });
                let usage = envelope.usage.map(|u| ProviderUsage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                });
                response.usage = usage.clone();
                return Ok((response, usage));
            }
            Some("max_tokens") => {
                return Err(AnthropicFailure {
                    class: AnthropicFailureClass::IncompleteResponse,
                    message: "response truncated by max_tokens; fails closed".into(),
                    retry_after_secs: None,
                });
            }
            Some(other) => {
                return Err(AnthropicFailure {
                    class: AnthropicFailureClass::IncompleteResponse,
                    message: format!("unsupported stop_reason: {other}"),
                    retry_after_secs: None,
                });
            }
            None => {
                return Err(AnthropicFailure {
                    class: AnthropicFailureClass::MalformedResponse,
                    message: "response carries no stop_reason".into(),
                    retry_after_secs: None,
                });
            }
        }
        let combined = join_text_content(&envelope.content);
        if combined.trim().is_empty() {
            return Err(AnthropicFailure {
                class: AnthropicFailureClass::MalformedResponse,
                message: "empty model output".into(),
                retry_after_secs: None,
            });
        }
        let wire: WireAgentResponse =
            serde_json::from_str(&combined).map_err(|e| AnthropicFailure {
                class: AnthropicFailureClass::MalformedResponse,
                message: format!("structured payload decode failed: {e}"),
                retry_after_secs: None,
            })?;
        let mut response =
            crate::openai_responses::map_wire_to_agent_response(wire).map_err(|e| {
                AnthropicFailure {
                    class: AnthropicFailureClass::MalformedResponse,
                    message: format!("structured payload rejected: {}", e.message),
                    retry_after_secs: None,
                }
            })?;
        let usage = envelope.usage.map(|u| ProviderUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        });
        response.usage = usage.clone();
        Ok((response, usage))
    }
}

fn agent_json_system_prompt() -> &'static str {
    "You are a reasoning assistant inside Omen. Omen establishes machine truth; \
    you reason over the supplied structured context; authority remains with Omen/Tethers. \
    Return ONLY JSON with keys kind (explanation|proposal|action_request|result|question|refusal), \
    message, proposed_actions, references, uncertainty. Do not claim you executed anything. \
    Proposals are typed intents, not permissions."
}

fn join_text_content(content: &[Value]) -> String {
    let mut parts = Vec::new();
    for item in content {
        let is_text = item.get("type").and_then(Value::as_str) == Some("text");
        if is_text && let Some(t) = item.get("text").and_then(Value::as_str) {
            parts.push(t.to_string());
        }
    }
    parts.join("")
}

fn classify_messages_error(kind: &str, message: &str) -> AnthropicFailure {
    match kind {
        "authentication_error" | "permission_error" => AnthropicFailure {
            class: AnthropicFailureClass::AuthFailure,
            message: message.to_string(),
            retry_after_secs: None,
        },
        "not_found_error" => AnthropicFailure {
            class: AnthropicFailureClass::ModelNotFound,
            message: message.to_string(),
            retry_after_secs: None,
        },
        "rate_limit_error" => AnthropicFailure {
            class: AnthropicFailureClass::RateLimited,
            message: message.to_string(),
            retry_after_secs: None,
        },
        "invalid_request_error" | "bad_request_error" => AnthropicFailure {
            class: AnthropicFailureClass::EndpointUnsupported,
            message: message.to_string(),
            retry_after_secs: None,
        },
        _ => AnthropicFailure {
            class: AnthropicFailureClass::ProviderFailure,
            message: format!("{kind}: {message}"),
            retry_after_secs: None,
        },
    }
}

fn classify_http_status(
    status: u16,
    body: &str,
    retry_after_secs: Option<u64>,
) -> Result<(), AnthropicFailure> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    if let Ok(envelope) = serde_json::from_str::<MessagesEnvelope>(body)
        && let Some(error) = envelope.error
    {
        let mut failure = classify_messages_error(&error.kind, &error.message);
        if failure.class == AnthropicFailureClass::RateLimited {
            failure.retry_after_secs = retry_after_secs;
        }
        return Err(failure);
    }
    let (class, message) = match status {
        401 => (
            AnthropicFailureClass::AuthFailure,
            "authentication failed without a typed body",
        ),
        404 => (
            AnthropicFailureClass::ModelNotFound,
            "endpoint or model not found",
        ),
        429 => (
            AnthropicFailureClass::RateLimited,
            "rate limited without a typed body",
        ),
        _ => (
            AnthropicFailureClass::ProviderFailure,
            "provider failure without a typed body",
        ),
    };
    Err(AnthropicFailure {
        class,
        message: message.to_string(),
        retry_after_secs,
    })
}

fn redactable_secret(secret: Option<&str>) -> Option<&str> {
    secret.filter(|s| s.len() >= 8)
}

impl AgentProvider for AnthropicSonnetProvider {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(async move {
            let body_value = self.build_request_body(&request)?;
            let body = serde_json::to_string(&body_value)
                .map_err(|e| AgentError::Rejected(format!("request serialization failed: {e}")))?;

            let provider = self.clone();
            let started = std::time::Instant::now();
            let snap = tokio::task::spawn_blocking(move || provider.post(&body))
                .await
                .map_err(|e| AgentError::Provider(format!("provider worker failed: {e}")))?
                .map_err(|f| self.safe_error(f))?;
            let latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

            classify_http_status(snap.status, &snap.body, snap.retry_after_secs)
                .map_err(|f| self.safe_error(f))?;

            let safe_body = bounded_redacted(&snap.body, self.api_key.as_deref());
            Self::parse_messages_body(&safe_body)
                .map(|(mut response, _)| {
                    response.latency_ms = Some(latency_ms);
                    response
                })
                .map_err(|f| self.safe_error(f))
        })
    }

    fn provider_identity(&self) -> ProviderIdentity {
        ProviderIdentity {
            id: self.provider_id.clone(),
            model: Some(self.config.model.clone()),
            effort: None,
            transport: Some("live-https".to_string()),
        }
    }
}

fn bounded_redacted(text: &str, secret: Option<&str>) -> String {
    const MAX_CHARS: usize = 64_000;
    let mut bounded: String = text.chars().take(MAX_CHARS).collect();
    if let Some(secret) = redactable_secret(secret) {
        bounded = bounded.replace(secret, "[REDACTED]");
    }
    bounded
}

/// Builds the Sonnet registry descriptor without exposing credentials.
pub fn anthropic_sonnet_descriptor(
    model: impl Into<String>,
    available: bool,
) -> ProviderDescriptor {
    ProviderDescriptor {
        id: ANTHROPIC_SONNET_PROVIDER_ID.into(),
        name: "Anthropic Claude Sonnet".into(),
        model: Some(model.into()),
        credential_source: Some(anthropic_credential_source()),
        capabilities: vec![
            ProviderCapability::Reasoning.as_str().into(),
            ProviderCapability::StructuredResponse.as_str().into(),
            ProviderCapability::FailureDiagnosis.as_str().into(),
            ProviderCapability::Navigation.as_str().into(),
            ProviderCapability::Proposal.as_str().into(),
            ProviderCapability::ToolProposal.as_str().into(),
        ],
        is_available: available,
    }
}

/// Sonnet preset transport wired with local credential presence only.
pub fn anthropic_sonnet_provider() -> AnthropicSonnetProvider {
    AnthropicSonnetProvider::with_config(
        AnthropicConfig::new(ANTHROPIC_SONNET_MODEL),
        anthropic_api_key_from_env(),
        ANTHROPIC_SONNET_PROVIDER_ID.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AgentContext;
    use crate::provider::{AgentResponseKind, AgentTurn};
    use omen_core::InteractiveSessionId;
    use std::path::PathBuf;
    use std::sync::Mutex;

    struct FixedHttp {
        status: u16,
        body: String,
        retry_after: Option<u64>,
        seen: Mutex<Vec<(String, String)>>,
    }

    impl FixedHttp {
        fn new(status: u16, body: &str, retry_after: Option<u64>) -> Self {
            Self {
                status,
                body: body.to_string(),
                retry_after,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl HttpPost for FixedHttp {
        fn post_json(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &str,
        ) -> Result<HttpResponseSnapshot, String> {
            let mut seen = self.seen.lock().unwrap();
            for (k, v) in headers {
                seen.push((k.clone(), v.clone()));
            }
            seen.push(("url".into(), url.into()));
            seen.push(("body".into(), body.into()));
            Ok(HttpResponseSnapshot {
                status: self.status,
                retry_after_secs: self.retry_after,
                body: self.body.clone(),
            })
        }
    }

    fn sample_request(prompt: &str) -> AgentRequest {
        AgentRequest {
            prompt: prompt.into(),
            context: AgentContext::new(
                "ws",
                PathBuf::from("/ws"),
                InteractiveSessionId::new("s1").unwrap(),
                PathBuf::from("/ws"),
            ),
            conversation: vec![AgentTurn {
                role: "user".into(),
                text: "hello".into(),
                timestamp: "2026-09-22T00:00:00Z".into(),
            }],
        }
    }

    fn sonnet_provider(http: Arc<dyn HttpPost>, key: Option<&str>) -> AnthropicSonnetProvider {
        AnthropicSonnetProvider::with_http(
            AnthropicConfig::new(ANTHROPIC_SONNET_MODEL),
            key.map(str::to_string),
            http,
            ANTHROPIC_SONNET_PROVIDER_ID,
        )
    }

    fn ok_text(inner: &str) -> String {
        json!({
            "type": "message",
            "id": "msg_mock",
            "model": ANTHROPIC_SONNET_MODEL,
            "role": "assistant",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": inner}],
            "usage": {"input_tokens": 21, "output_tokens": 9},
        })
        .to_string()
    }

    fn ok_agent_json(kind: &str, message: &str) -> String {
        json!({
            "kind": kind,
            "message": message,
            "proposed_actions": [],
            "references": [],
            "uncertainty": null
        })
        .to_string()
    }

    fn posts_of(http: &FixedHttp) -> usize {
        http.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == "url")
            .count()
    }

    #[test]
    fn sonnet_identity_is_verified_preset() {
        assert_eq!(ANTHROPIC_SONNET_MODEL, "claude-sonnet-5-5");
        let provider = sonnet_provider(Arc::new(FixedHttp::new(200, "{}", None)), Some("k"));
        assert_eq!(provider.model(), "claude-sonnet-5-5");
        assert_eq!(provider.provider_id(), ANTHROPIC_SONNET_PROVIDER_ID);
        let identity = provider.provider_identity();
        assert_eq!(identity.id, ANTHROPIC_SONNET_PROVIDER_ID);
        assert_eq!(identity.model.as_deref(), Some("claude-sonnet-5-5"));
        let desc = anthropic_sonnet_descriptor("claude-sonnet-5-5", true);
        assert_eq!(desc.id, ANTHROPIC_SONNET_PROVIDER_ID);
        assert!(desc.is_available);
        assert!(desc.capabilities.contains(&"reasoning".to_string()));
    }

    #[test]
    fn debug_never_prints_api_key() {
        let provider = sonnet_provider(
            Arc::new(FixedHttp::new(200, "{}", None)),
            Some("sk-ant-super-secret"),
        );
        let dbg = format!("{provider:?}");
        assert!(!dbg.contains("sk-ant-super-secret"));
        assert!(dbg.contains("<redacted>"));
        assert!(dbg.contains("claude-sonnet-5-5"));
    }

    #[test]
    fn request_body_uses_messages_contract() {
        let http = Arc::new(FixedHttp::new(200, "{}", None));
        let provider = sonnet_provider(http.clone(), Some("k"));
        let body = provider.build_request_body(&sample_request("hi")).unwrap();
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(body["max_tokens"], ANTHROPIC_DEFAULT_MAX_TOKENS);
        assert!(body["system"].as_str().unwrap().contains("JSON"));
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn success_reports_usage_latency_and_single_attempt() {
        let http = Arc::new(FixedHttp::new(
            200,
            &ok_text(&ok_agent_json("explanation", "done")),
            None,
        ));
        let provider = sonnet_provider(http.clone(), Some("k"));
        let response = provider.respond(sample_request("x")).await.unwrap();
        assert_eq!(response.kind, AgentResponseKind::Explanation);
        assert_eq!(response.message, "done");
        let usage = response.usage.expect("usage attaches");
        assert_eq!(usage.input_tokens, 21);
        assert_eq!(usage.output_tokens, 9);
        assert!(response.latency_ms.is_some());
        assert_eq!(posts_of(&http), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auth_failure_is_authentication_required() {
        for (status, body) in [
            (
                401,
                r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
            ),
            (401, "not json at all"),
        ] {
            let provider = sonnet_provider(Arc::new(FixedHttp::new(status, body, None)), Some("k"));
            match provider.respond(sample_request("x")).await.unwrap_err() {
                AgentError::AuthenticationRequired { provider, .. } => {
                    assert_eq!(provider, ANTHROPIC_SONNET_PROVIDER_ID)
                }
                other => panic!("{other:?}"),
            }
        }
        // Missing credential fails before any POST.
        let http = Arc::new(FixedHttp::new(200, "{}", None));
        let provider = sonnet_provider(http.clone(), None);
        match provider.respond(sample_request("x")).await.unwrap_err() {
            AgentError::AuthenticationRequired { message, .. } => {
                assert!(message.contains(ANTHROPIC_API_KEY_ENV))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(posts_of(&http), 0, "missing credential posts nothing");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn malformed_bodies_fail_closed() {
        for body in [
            "not json".to_string(),
            json!({"type": "message"}).to_string(),
            json!({
                "type": "message", "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "not agent json"}],
            })
            .to_string(),
            json!({
                "type": "message", "stop_reason": "end_turn",
                "content": [],
            })
            .to_string(),
        ] {
            let provider = sonnet_provider(Arc::new(FixedHttp::new(200, &body, None)), Some("k"));
            match provider.respond(sample_request("x")).await.unwrap_err() {
                AgentError::Rejected(_) => {}
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refusal_stop_reason_becomes_typed_refusal() {
        let body = json!({
            "type": "message", "stop_reason": "refusal",
            "content": [{"type": "text", "text": "I cannot help with that."}],
            "usage": {"input_tokens": 5, "output_tokens": 6},
        })
        .to_string();
        let provider = sonnet_provider(Arc::new(FixedHttp::new(200, &body, None)), Some("k"));
        let response = provider.respond(sample_request("x")).await.unwrap();
        assert_eq!(response.kind, AgentResponseKind::Refusal);
        assert!(response.message.contains("cannot help"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rate_limit_and_timeout_map_without_retry() {
        let http = Arc::new(FixedHttp::new(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#,
            Some(9),
        ));
        let provider = sonnet_provider(http.clone(), Some("k"));
        match provider.respond(sample_request("x")).await.unwrap_err() {
            AgentError::RateLimited {
                retry_after_secs, ..
            } => {
                assert_eq!(retry_after_secs, Some(9))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(posts_of(&http), 1, "no billable retry");

        let http = Arc::new(FixedHttp::new(500, "boom", None));
        let provider = sonnet_provider(http.clone(), Some("k"));
        match provider.respond(sample_request("x")).await.unwrap_err() {
            AgentError::Provider(_) => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(posts_of(&http), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn transport_timeout_maps_to_timeout() {
        struct TimeoutHttp;
        impl HttpPost for TimeoutHttp {
            fn post_json(
                &self,
                _url: &str,
                _headers: &[(String, String)],
                _body: &str,
            ) -> Result<HttpResponseSnapshot, String> {
                Err("connection timeout after 30s".into())
            }
        }
        let provider = sonnet_provider(Arc::new(TimeoutHttp), Some("k"));
        match provider.respond(sample_request("x")).await.unwrap_err() {
            AgentError::Timeout(_) => {}
            other => panic!("{other:?}"),
        }
    }
}
