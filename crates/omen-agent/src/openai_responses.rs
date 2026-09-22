use crate::context::AgentContext;
use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind,
    DEFAULT_AGENT_TIMEOUT, ProposedAction,
};
use crate::registry::ProviderDescriptor;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::future::Future;
use std::io::Read;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// User-facing Luna preset identity (registry-facing; not the transport name).
pub const OPENAI_LUNA_PROVIDER_ID: &str = "openai-luna";
/// Current Omen OpenAI Platform project policy preset.
pub const OPENAI_LUNA_MODEL: &str = "gpt-6-luna";
pub const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
pub const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
pub const OPENAI_DEFAULT_REASONING_EFFORT: &str = "medium";
pub const OPENAI_DEFAULT_MAX_OUTPUT_TOKENS: u32 = 800;
const MAX_CONTEXT_JSON_BYTES: usize = 48 * 1024;
const MAX_MESSAGE_CHARS: usize = 8_000;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ACTIONS: usize = 32;
const MAX_REFERENCE_CHARS: usize = 512;

/// Credential source reported on the descriptor; never contains the secret itself.
pub fn openai_credential_source() -> String {
    format!("environment:{OPENAI_API_KEY_ENV}")
}

/// Reads the API key from the process environment when present and non-empty.
///
/// Credential presence only proves the provider is locally configured enough
/// to attempt use. It does not prove the key is valid, that the project is
/// entitled to a model, or that the network is reachable. Runtime HTTP truth
/// remains authoritative for those outcomes.
pub fn openai_api_key_from_env() -> Option<String> {
    match std::env::var(OPENAI_API_KEY_ENV) {
        Ok(k) if !k.trim().is_empty() => Some(k),
        _ => None,
    }
}

/// Builds the Luna preset registry descriptor without exposing credentials.
pub fn openai_luna_descriptor(model: impl Into<String>, available: bool) -> ProviderDescriptor {
    ProviderDescriptor {
        id: OPENAI_LUNA_PROVIDER_ID.into(),
        name: "OpenAI GPT-6 Luna".into(),
        model: Some(model.into()),
        credential_source: Some(openai_credential_source()),
        capabilities: vec![
            "reasoning".into(),
            "structured-response".into(),
            "failure-diagnosis".into(),
            "navigation".into(),
            "proposal".into(),
            "tool-proposal".into(),
        ],
        // Locally configured enough to attempt use; remote entitlement is not claimed.
        is_available: available,
    }
}

/// Outbound HTTP response reduced to what the provider needs.
#[derive(Debug, Clone)]
pub struct HttpResponseSnapshot {
    pub status: u16,
    pub retry_after_secs: Option<u64>,
    pub body: String,
}

/// Pluggable HTTP POST used for tests and production.
pub trait HttpPost: Send + Sync + 'static {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponseSnapshot, String>;
}

/// Production transport: single bounded HTTPS POST via ureq (no shell-outs).
#[derive(Debug, Clone)]
pub struct UreqHttpPost {
    timeout: Duration,
}

impl UreqHttpPost {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .timeout_connect(Some(self.timeout))
            .timeout_send_request(Some(self.timeout))
            .timeout_recv_response(Some(self.timeout))
            .timeout_recv_body(Some(self.timeout))
            .http_status_as_error(false)
            .build()
            .new_agent()
    }
}

impl HttpPost for UreqHttpPost {
    fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponseSnapshot, String> {
        let mut req = self
            .agent()
            .post(url)
            .header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        match req.send(body) {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let retry_after_secs = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok());
                let reader = resp.into_body().into_reader();
                let mut bytes = Vec::with_capacity(8 * 1024);
                reader
                    .take((MAX_RESPONSE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("response body read failed: {e}"))?;
                if bytes.len() > MAX_RESPONSE_BYTES {
                    return Err(format!(
                        "provider response exceeded {MAX_RESPONSE_BYTES} byte limit"
                    ));
                }
                let text = String::from_utf8(bytes)
                    .map_err(|_| "provider response body was not UTF-8".to_string())?;
                Ok(HttpResponseSnapshot {
                    status,
                    retry_after_secs,
                    body: text,
                })
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.to_lowercase().contains("timeout")
                    || msg.to_lowercase().contains("timed out")
                {
                    Err(format!("timeout: {msg}"))
                } else {
                    Err(msg)
                }
            }
        }
    }
}

/// Model-neutral configuration for the OpenAI Responses transport.
#[derive(Debug, Clone)]
pub struct OpenAiResponsesConfig {
    pub model: String,
    pub responses_url: String,
    pub reasoning_effort: Option<String>,
    pub timeout: Duration,
    pub max_output_tokens: u32,
}

impl OpenAiResponsesConfig {
    /// Neutral default: standard endpoint, default effort/timeout/budget.
    /// Callers supply an explicit model for any concrete preset.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            responses_url: OPENAI_RESPONSES_URL.to_string(),
            reasoning_effort: Some(OPENAI_DEFAULT_REASONING_EFFORT.to_string()),
            timeout: DEFAULT_AGENT_TIMEOUT,
            max_output_tokens: OPENAI_DEFAULT_MAX_OUTPUT_TOKENS,
        }
    }

    /// Current Omen Luna preset over the same transport.
    pub fn luna_preset() -> Self {
        Self::new(OPENAI_LUNA_MODEL)
    }
}

/// Wire shapes for OpenAI Responses structured output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireAgentResponse {
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub proposed_actions: Vec<Value>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub uncertainty: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesApiEnvelope {
    status: Option<String>,
    output: Option<Vec<Value>>,
    error: Option<Value>,
    incomplete_details: Option<IncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct IncompleteDetails {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    message: String,
}

/// Errors classified for tests and mapping into AgentError.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAiFailureClass {
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
    /// Responses HTTP 200 body was not a completed AgentResponse.
    IncompleteResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiFailure {
    pub class: OpenAiFailureClass,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl OpenAiFailure {
    pub fn to_agent_error(&self, provider: &str) -> AgentError {
        match self.class {
            OpenAiFailureClass::MissingCredential | OpenAiFailureClass::AuthFailure => {
                AgentError::AuthenticationRequired {
                    provider: provider.to_string(),
                    message: self.message.clone(),
                }
            }
            OpenAiFailureClass::AccountNotEntitled
            | OpenAiFailureClass::ModelNotFound
            | OpenAiFailureClass::EndpointUnsupported => AgentError::ProviderUnavailable {
                provider: provider.to_string(),
                message: self.message.clone(),
            },
            OpenAiFailureClass::RateLimited => AgentError::RateLimited {
                provider: provider.to_string(),
                retry_after_secs: self.retry_after_secs,
            },
            OpenAiFailureClass::Timeout => AgentError::Timeout(
                self.retry_after_secs
                    .map(Duration::from_secs)
                    .unwrap_or(DEFAULT_AGENT_TIMEOUT),
            ),
            OpenAiFailureClass::UnsupportedFeature => AgentError::UnsupportedCapability {
                provider: provider.to_string(),
                capability: self.message.clone(),
            },
            OpenAiFailureClass::MalformedResponse => AgentError::Rejected(format!(
                "provider '{provider}' returned a malformed structured response: {}",
                self.message
            )),
            OpenAiFailureClass::IncompleteResponse => AgentError::Rejected(format!(
                "provider '{provider}' response not completed: {}",
                self.message
            )),
            OpenAiFailureClass::ProviderFailure => {
                AgentError::Provider(format!("provider '{provider}': {}", self.message))
            }
        }
    }
}

fn classify_http(status: u16, body: &str, retry_after: Option<u64>) -> Result<(), OpenAiFailure> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let detail = parse_error_message(body);
    let lower = body.to_lowercase();
    match status {
        401 => Err(OpenAiFailure {
            class: OpenAiFailureClass::AuthFailure,
            message: detail.unwrap_or_else(|| "unauthorized".into()),
            retry_after_secs: retry_after,
        }),
        403 => {
            if lower.contains("not entitled")
                || lower.contains("permission")
                || lower.contains("does not have access")
            {
                Err(OpenAiFailure {
                    class: OpenAiFailureClass::AccountNotEntitled,
                    message: detail.unwrap_or_else(|| "account not entitled".into()),
                    retry_after_secs: retry_after,
                })
            } else {
                Err(OpenAiFailure {
                    class: OpenAiFailureClass::AuthFailure,
                    message: detail.unwrap_or_else(|| "forbidden".into()),
                    retry_after_secs: retry_after,
                })
            }
        }
        404 | 400
            if lower.contains("model")
                && (lower.contains("not found")
                    || lower.contains("does not exist")
                    || lower.contains("invalid model")) =>
        {
            Err(OpenAiFailure {
                class: OpenAiFailureClass::ModelNotFound,
                message: detail.unwrap_or_else(|| "model not found".into()),
                retry_after_secs: retry_after,
            })
        }
        400 if lower.contains("reasoning")
            || lower.contains("unsupported")
            || lower.contains("unknown parameter")
            || lower.contains("invalid_request") =>
        {
            Err(OpenAiFailure {
                class: OpenAiFailureClass::UnsupportedFeature,
                message: detail.unwrap_or_else(|| "unsupported request feature".into()),
                retry_after_secs: retry_after,
            })
        }
        404 => Err(OpenAiFailure {
            class: OpenAiFailureClass::EndpointUnsupported,
            message: detail.unwrap_or_else(|| "endpoint not found".into()),
            retry_after_secs: retry_after,
        }),
        429 => Err(OpenAiFailure {
            class: OpenAiFailureClass::RateLimited,
            message: detail.unwrap_or_else(|| "rate limited".into()),
            retry_after_secs: retry_after,
        }),
        500..=599 => Err(OpenAiFailure {
            class: OpenAiFailureClass::ProviderFailure,
            message: detail.unwrap_or_else(|| format!("upstream status {status}")),
            retry_after_secs: retry_after,
        }),
        _ => Err(OpenAiFailure {
            class: OpenAiFailureClass::ProviderFailure,
            message: detail.unwrap_or_else(|| format!("http {status}")),
            retry_after_secs: retry_after,
        }),
    }
}

fn parse_error_message(body: &str) -> Option<String> {
    let parsed: ErrorBody = serde_json::from_str(body).ok()?;
    Some(parsed.error.message)
}

fn incomplete_failure(message: String) -> OpenAiFailure {
    OpenAiFailure {
        class: OpenAiFailureClass::IncompleteResponse,
        message,
        retry_after_secs: None,
    }
}

/// Enforce Responses status truth: only `completed` may become an AgentResponse.
fn ensure_completed_status(envelope: &ResponsesApiEnvelope) -> Result<(), OpenAiFailure> {
    let Some(status) = envelope.status.as_deref() else {
        return Err(incomplete_failure("missing response status".into()));
    };

    match status {
        "completed" => Ok(()),
        "incomplete" => {
            let reason = envelope
                .incomplete_details
                .as_ref()
                .and_then(|d| d.reason.clone())
                .unwrap_or_else(|| "unknown".into());
            Err(incomplete_failure(format!(
                "status=incomplete reason={reason}"
            )))
        }
        "failed" => {
            let detail = extract_error_detail(envelope.error.as_ref())
                .unwrap_or_else(|| "provider reported failed".into());
            Err(incomplete_failure(format!("status=failed error={detail}")))
        }
        "cancelled" => Err(incomplete_failure("status=cancelled".into())),
        "queued" => Err(incomplete_failure(
            "status=queued (unexpected for synchronous provider)".into(),
        )),
        "in_progress" => Err(incomplete_failure(
            "status=in_progress (unexpected for synchronous provider)".into(),
        )),
        other => Err(incomplete_failure(format!(
            "unsupported response status '{other}'"
        ))),
    }
}

fn extract_error_detail(error: Option<&Value>) -> Option<String> {
    let err = error?;
    if let Some(obj) = err.as_object() {
        if let Some(message) = obj
            .get("message")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Some(bounded_and_redacted(message, None, 512));
        }
        // Nested {"error": {...}} shapes
        if let Some(nested) = obj.get("error") {
            return extract_error_detail(Some(nested));
        }
    }
    if let Some(s) = err.as_str().filter(|s| !s.trim().is_empty()) {
        return Some(bounded_and_redacted(s, None, 512));
    }
    None
}

/// Model-neutral OpenAI Responses AgentProvider.
///
/// Transport identity is OpenAI Responses; provider/preset identity
/// (e.g. `openai-luna`) is supplied separately for AgentError labels.
///
/// Omen remains substrate: this provider only proposes typed actions.
/// It never executes model-produced text and never places the API key in
/// AgentContext, Debug output, or Display output.
#[derive(Clone)]
pub struct OpenAiResponsesProvider {
    config: OpenAiResponsesConfig,
    api_key: Option<String>,
    http: Arc<dyn HttpPost>,
    /// Provider identity used in AgentError labels (preset id, not transport name).
    provider_id: String,
}

impl std::fmt::Debug for OpenAiResponsesProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiResponsesProvider")
            .field("provider_id", &self.provider_id)
            .field("model", &self.config.model)
            .field("responses_url", &self.config.responses_url)
            .field("reasoning_effort", &self.config.reasoning_effort)
            .field("timeout", &self.config.timeout)
            .field("max_output_tokens", &self.config.max_output_tokens)
            .field("credential", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("has_credential", &self.api_key.is_some())
            .finish()
    }
}

impl OpenAiResponsesProvider {
    /// Luna preset constructor: transport + model + provider id together.
    pub fn luna_preset() -> Self {
        Self::with_config(
            OpenAiResponsesConfig::luna_preset(),
            openai_api_key_from_env(),
            OPENAI_LUNA_PROVIDER_ID.to_string(),
        )
    }

    /// Preset-shaped constructor from environment (Luna default model).
    pub fn from_env() -> Self {
        Self::luna_preset()
    }

    pub fn with_config(
        config: OpenAiResponsesConfig,
        api_key: Option<String>,
        provider_id: impl Into<String>,
    ) -> Self {
        let timeout = config.timeout;
        Self {
            config,
            api_key,
            http: Arc::new(UreqHttpPost::new(timeout)),
            provider_id: provider_id.into(),
        }
    }

    pub fn with_http(
        config: OpenAiResponsesConfig,
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
        self.api_key.is_some()
    }

    pub fn descriptor(&self) -> ProviderDescriptor {
        openai_luna_descriptor(self.config.model.clone(), self.has_credential())
    }

    /// Builds a bounded structured request body (never includes credentials).
    pub fn build_request_body(&self, request: &AgentRequest) -> Result<Value, AgentError> {
        let mut context_json = serde_json::to_value(&request.context).map_err(|e| {
            AgentError::Rejected(format!("agent context serialization failed: {e}"))
        })?;
        redact_json_strings(&mut context_json, self.api_key.as_deref());
        let context_str = serde_json::to_string(&context_json).unwrap_or_default();
        if context_str.len() > MAX_CONTEXT_JSON_BYTES {
            context_json = json!({
                "truncated": true,
                "workspace_root": request.context.workspace_root,
                "cwd": request.context.cwd,
                "note": "bounded AgentContext omitted oversized payload",
            });
        }

        let message = AgentContext::bounded_excerpt(&request.prompt, MAX_MESSAGE_CHARS);

        let mut system = String::from(
            "You are a reasoning assistant inside Omen. Omen establishes machine truth; \
you reason over the supplied structured context; authority remains with Omen/Tethers. \
Return ONLY JSON matching the required schema. Do not claim you executed anything. \
Proposals are typed intents, not permissions. If machine truth already answers the \
question, prefer a short explanation over inventing actions.",
        );
        if !request.conversation.is_empty() {
            system.push_str("\nRecent conversation:");
            for turn in request.conversation.iter().rev().take(6).rev() {
                system.push_str(&format!(
                    "\n{}: {}",
                    bounded_and_redacted(&turn.role, self.api_key.as_deref(), 64),
                    bounded_and_redacted(&turn.text, self.api_key.as_deref(), 2_000)
                ));
            }
        }

        let mut body = json!({
            "model": self.config.model,
            "input": [
                {
                    "role": "system",
                    "content": [{ "type": "input_text", "text": system }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": format!(
                            "AgentContext:\n{}\n\nUser prompt:\n{}",
                            serde_json::to_string_pretty(&context_json)
                                .unwrap_or_else(|_| "{}".into()),
                            bounded_and_redacted(&message, self.api_key.as_deref(), MAX_MESSAGE_CHARS)
                        )
                    }]
                }
            ],
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "agent_response",
                    "strict": true,
                    "schema": agent_response_json_schema()
                }
            },
            "max_output_tokens": self.config.max_output_tokens
        });

        if let Some(effort) = &self.config.reasoning_effort {
            body["reasoning"] = json!({ "effort": effort });
        }

        Ok(body)
    }

    /// Parses a Responses API body into a typed AgentResponse.
    ///
    /// Only `status == completed` may produce an AgentResponse.
    pub fn parse_responses_body(body: &str) -> Result<AgentResponse, OpenAiFailure> {
        let envelope: ResponsesApiEnvelope =
            serde_json::from_str(body).map_err(|e| OpenAiFailure {
                class: OpenAiFailureClass::MalformedResponse,
                message: format!("envelope decode failed: {e}"),
                retry_after_secs: None,
            })?;

        ensure_completed_status(&envelope)?;

        let mut text_parts: Vec<String> = Vec::new();
        if let Some(output) = envelope.output {
            for item in output {
                if item.get("type").and_then(Value::as_str) == Some("message")
                    && let Some(content) = item.get("content").and_then(Value::as_array)
                {
                    for part in content {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            text_parts.push(t.to_string());
                        }
                    }
                }
            }
        }

        let combined = text_parts.join("");
        if combined.trim().is_empty() {
            return Err(OpenAiFailure {
                class: OpenAiFailureClass::MalformedResponse,
                message: "empty model output".into(),
                retry_after_secs: None,
            });
        }

        let wire: WireAgentResponse =
            serde_json::from_str(&combined).map_err(|e| OpenAiFailure {
                class: OpenAiFailureClass::MalformedResponse,
                message: format!("structured payload decode failed: {e}"),
                retry_after_secs: None,
            })?;

        map_wire_to_agent_response(wire)
    }

    fn post(&self, body: &str) -> Result<HttpResponseSnapshot, OpenAiFailure> {
        let api_key = self.api_key.clone().ok_or(OpenAiFailure {
            class: OpenAiFailureClass::MissingCredential,
            message: format!("missing credential {OPENAI_API_KEY_ENV}"),
            retry_after_secs: None,
        })?;

        // Content-Type is set once by the transport; only Authorization is caller-supplied.
        let headers = vec![("authorization".to_string(), format!("Bearer {api_key}"))];

        match self
            .http
            .post_json(&self.config.responses_url, &headers, body)
        {
            Ok(resp) => Ok(resp),
            Err(e) if e.to_lowercase().contains("timeout") => Err(OpenAiFailure {
                class: OpenAiFailureClass::Timeout,
                message: e,
                retry_after_secs: Some(self.config.timeout.as_secs()),
            }),
            Err(e) => Err(OpenAiFailure {
                class: OpenAiFailureClass::ProviderFailure,
                message: e,
                retry_after_secs: None,
            }),
        }
    }

    fn safe_error(&self, mut failure: OpenAiFailure) -> AgentError {
        if let Some(secret) = redactable_secret(self.api_key.as_deref()) {
            failure.message = failure.message.replace(secret, "[REDACTED]");
        }
        failure.to_agent_error(&self.provider_id)
    }
}

impl AgentProvider for OpenAiResponsesProvider {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(async move {
            let body_value = self.build_request_body(&request)?;
            let body = serde_json::to_string(&body_value)
                .map_err(|e| AgentError::Rejected(format!("request serialization failed: {e}")))?;

            let provider = self.clone();
            let snap = tokio::task::spawn_blocking(move || provider.post(&body))
                .await
                .map_err(|e| AgentError::Provider(format!("provider worker failed: {e}")))?
                .map_err(|f| self.safe_error(f))?;

            classify_http(snap.status, &snap.body, snap.retry_after_secs)
                .map_err(|f| self.safe_error(f))?;

            let safe_body =
                bounded_and_redacted(&snap.body, self.api_key.as_deref(), MAX_RESPONSE_BYTES);
            Self::parse_responses_body(&safe_body).map_err(|f| self.safe_error(f))
        })
    }
}

/// Only redact secrets long enough to be real credentials. Short test keys
/// like "k" must not rewrite field names such as `"kind"`.
fn redactable_secret(secret: Option<&str>) -> Option<&str> {
    secret.filter(|s| s.len() >= 8)
}

fn bounded_and_redacted(text: &str, secret: Option<&str>, max_chars: usize) -> String {
    let mut bounded = text.chars().take(max_chars).collect::<String>();
    if let Some(secret) = redactable_secret(secret) {
        bounded = bounded.replace(secret, "[REDACTED]");
    }
    bounded
}

fn redact_json_strings(value: &mut Value, secret: Option<&str>) {
    let Some(secret) = redactable_secret(secret) else {
        return;
    };
    match value {
        Value::String(text) => {
            *text = text.replace(secret, "[REDACTED]");
        }
        Value::Array(items) => {
            for item in items {
                redact_json_strings(item, Some(secret));
            }
        }
        Value::Object(fields) => {
            for item in fields.values_mut() {
                redact_json_strings(item, Some(secret));
            }
        }
        _ => {}
    }
}

/// Strict JSON schema for structured Responses output.
pub fn agent_response_json_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["explanation", "proposal", "action_request", "result", "question", "refusal"]
            },
            "message": { "type": "string" },
            "proposed_actions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "action_type": {
                            "type": "string",
                            "enum": ["change_directory", "execute_tool", "execute_command", "semantic_action"]
                        },
                        "path": { "type": ["string", "null"] },
                        "tool": { "type": ["string", "null"] },
                        "operation": { "type": ["string", "null"] },
                        "args": { "type": "array", "items": { "type": "string" } },
                        "cwd": { "type": ["string", "null"] },
                        "argv": { "type": "array", "items": { "type": "string" } },
                        "action": { "type": ["string", "null"] }
                    },
                    "required": ["action_type", "path", "tool", "operation", "args", "cwd", "argv", "action"]
                }
            },
            "references": { "type": "array", "items": { "type": "string" } },
            "uncertainty": { "type": ["string", "null"] }
        },
        "required": ["kind", "message", "proposed_actions", "references", "uncertainty"]
    })
}

fn map_kind(raw: &str) -> Result<AgentResponseKind, OpenAiFailure> {
    match raw {
        "explanation" => Ok(AgentResponseKind::Explanation),
        "proposal" => Ok(AgentResponseKind::Proposal),
        "action_request" => Ok(AgentResponseKind::ActionRequest),
        "result" => Ok(AgentResponseKind::Result),
        "question" => Ok(AgentResponseKind::Question),
        "refusal" => Ok(AgentResponseKind::Refusal),
        other => Err(OpenAiFailure {
            class: OpenAiFailureClass::MalformedResponse,
            message: format!("unknown response kind '{other}'"),
            retry_after_secs: None,
        }),
    }
}

fn map_action(v: &Value) -> Result<ProposedAction, OpenAiFailure> {
    let action_type = v
        .get("action_type")
        .and_then(Value::as_str)
        .ok_or_else(|| OpenAiFailure {
            class: OpenAiFailureClass::MalformedResponse,
            message: "action missing action_type".into(),
            retry_after_secs: None,
        })?;

    let opt_str = |k: &str| -> Result<Option<String>, OpenAiFailure> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.chars().count() <= MAX_MESSAGE_CHARS => Ok(Some(s.clone())),
            Some(Value::String(_)) => Err(malformed_action(&format!("{k} exceeds size limit"))),
            Some(_) => Err(malformed_action(&format!("{k} must be a string or null"))),
        }
    };
    let str_list = |k: &str| -> Result<Vec<String>, OpenAiFailure> {
        let Some(Value::Array(items)) = v.get(k) else {
            return Err(malformed_action(&format!("{k} must be a string array")));
        };
        items
            .iter()
            .map(|item| match item {
                Value::String(s) if s.chars().count() <= MAX_MESSAGE_CHARS => Ok(s.clone()),
                Value::String(_) => Err(malformed_action(&format!("{k} item exceeds size limit"))),
                _ => Err(malformed_action(&format!("{k} must contain only strings"))),
            })
            .collect()
    };

    match action_type {
        "change_directory" => {
            let path = opt_str("path")?
                .ok_or_else(|| malformed_action("change_directory missing path"))?;
            Ok(ProposedAction::ChangeDirectory {
                path: PathBuf::from(path),
            })
        }
        "execute_tool" => {
            let tool =
                opt_str("tool")?.ok_or_else(|| malformed_action("execute_tool missing tool"))?;
            Ok(ProposedAction::ExecuteTool {
                tool,
                operation: opt_str("operation")?.unwrap_or_default(),
                args: str_list("args")?,
                cwd: opt_str("cwd")?,
            })
        }
        "execute_command" => {
            let argv = str_list("argv")?;
            if argv.is_empty() {
                return Err(malformed_action("execute_command missing argv"));
            }
            Ok(ProposedAction::ExecuteCommand {
                argv,
                cwd: opt_str("cwd")?,
            })
        }
        "semantic_action" => {
            let action = opt_str("action")?
                .ok_or_else(|| malformed_action("semantic_action missing action"))?;
            Ok(ProposedAction::SemanticAction {
                action,
                args: str_list("args")?,
            })
        }
        other => Err(OpenAiFailure {
            class: OpenAiFailureClass::MalformedResponse,
            message: format!("unknown action_type '{other}'"),
            retry_after_secs: None,
        }),
    }
}

fn malformed_action(message: &str) -> OpenAiFailure {
    OpenAiFailure {
        class: OpenAiFailureClass::MalformedResponse,
        message: message.to_string(),
        retry_after_secs: None,
    }
}

fn map_wire_to_agent_response(wire: WireAgentResponse) -> Result<AgentResponse, OpenAiFailure> {
    let kind = map_kind(&wire.kind)?;

    if wire.proposed_actions.len() > MAX_ACTIONS {
        return Err(OpenAiFailure {
            class: OpenAiFailureClass::MalformedResponse,
            message: format!("too many proposed actions (maximum {MAX_ACTIONS})"),
            retry_after_secs: None,
        });
    }

    let mut actions = Vec::with_capacity(wire.proposed_actions.len());
    let mut action_errors = Vec::new();
    for raw in &wire.proposed_actions {
        match map_action(raw) {
            Ok(a) => actions.push(a),
            Err(e) => action_errors.push(e.message),
        }
    }

    // Authority boundary: every action element must validate, regardless of kind.
    if !action_errors.is_empty() {
        return Err(OpenAiFailure {
            class: OpenAiFailureClass::MalformedResponse,
            message: format!("malformed proposed actions: {}", action_errors.join("; ")),
            retry_after_secs: None,
        });
    }

    if wire.references.len() > 32
        || wire
            .references
            .iter()
            .any(|r| r.chars().count() > MAX_REFERENCE_CHARS)
        || wire.message.chars().count() > MAX_MESSAGE_CHARS
        || wire
            .uncertainty
            .as_ref()
            .is_some_and(|s| s.chars().count() > 1_000)
    {
        return Err(OpenAiFailure {
            class: OpenAiFailureClass::MalformedResponse,
            message: "structured response exceeded field bounds".into(),
            retry_after_secs: None,
        });
    }

    Ok(AgentResponse {
        kind,
        message: wire.message,
        proposed_actions: actions,
        references: wire.references,
        uncertainty: wire.uncertainty.filter(|s| !s.trim().is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AgentContext;
    use crate::provider::AgentTurn;
    use omen_core::InteractiveSessionId;
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

    fn luna_provider(http: Arc<dyn HttpPost>, key: Option<&str>) -> OpenAiResponsesProvider {
        OpenAiResponsesProvider::with_http(
            OpenAiResponsesConfig::luna_preset(),
            key.map(str::to_string),
            http,
            OPENAI_LUNA_PROVIDER_ID,
        )
    }

    fn ok_body(message: &str, kind: &str) -> String {
        json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": json!({
                        "kind": kind,
                        "message": message,
                        "proposed_actions": [],
                        "references": ["@last"],
                        "uncertainty": null
                    }).to_string()
                }]
            }]
        })
        .to_string()
    }

    fn envelope_with_status(status: &str, extra: Value) -> String {
        let mut obj = json!({ "status": status });
        if let Value::Object(map) = extra {
            for (k, v) in map {
                obj[k] = v;
            }
        }
        obj.to_string()
    }

    #[test]
    fn generic_config_emits_synthetic_model() {
        let cfg = OpenAiResponsesConfig::new("example-model");
        let provider = OpenAiResponsesProvider::with_http(
            cfg,
            Some("k".into()),
            Arc::new(FixedHttp::new(200, "{}", None)),
            "example-provider",
        );
        let body = provider.build_request_body(&sample_request("hi")).unwrap();
        assert_eq!(body["model"], "example-model");
        assert!(
            !body.to_string().contains("gpt-6-luna"),
            "synthetic model must not be rewritten to Luna"
        );
    }

    #[test]
    fn luna_preset_emits_gpt_6_luna() {
        let provider = OpenAiResponsesProvider::with_http(
            OpenAiResponsesConfig::luna_preset(),
            Some("k".into()),
            Arc::new(FixedHttp::new(200, "{}", None)),
            OPENAI_LUNA_PROVIDER_ID,
        );
        let body = provider.build_request_body(&sample_request("hi")).unwrap();
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["max_output_tokens"], 800);
        assert!(body.to_string().contains("json_schema"));
    }

    #[test]
    fn builds_bounded_request_without_credential() {
        let provider = OpenAiResponsesProvider::with_http(
            OpenAiResponsesConfig::luna_preset(),
            Some("sk-test-secret".into()),
            Arc::new(FixedHttp::new(200, "{}", None)),
            OPENAI_LUNA_PROVIDER_ID,
        );
        let body = provider
            .build_request_body(&sample_request("why exit 2?"))
            .unwrap();
        let s = body.to_string();
        assert!(s.contains("gpt-6-luna"));
        assert!(!s.contains("sk-test-secret"));
        assert!(s.contains("json_schema"));
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn request_redacts_credential_from_prompt_and_context() {
        let secret = "sk-test-secret";
        let provider = OpenAiResponsesProvider::with_http(
            OpenAiResponsesConfig::luna_preset(),
            Some(secret.into()),
            Arc::new(FixedHttp::new(200, "{}", None)),
            OPENAI_LUNA_PROVIDER_ID,
        );
        let mut request = sample_request(&format!("please inspect {secret}"));
        request.context.environment.username = secret.into();
        request.conversation[0].text = secret.into();

        let body = provider.build_request_body(&request).unwrap().to_string();
        assert!(!body.contains(secret));
        assert!(body.contains("[REDACTED]"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn errors_and_model_output_redact_credential() {
        let secret = "sk-test-secret";
        let error_http = Arc::new(FixedHttp::new(
            401,
            &format!(r#"{{"error":{{"message":"rejected {secret}"}}}}"#),
            None,
        ));
        let provider = luna_provider(error_http, Some(secret));
        let error = provider.respond(sample_request("x")).await.unwrap_err();
        assert!(!error.to_string().contains(secret));

        let success_http = Arc::new(FixedHttp::new(200, &ok_body(secret, "explanation"), None));
        let provider = luna_provider(success_http, Some(secret));
        let response = provider.respond(sample_request("x")).await.unwrap();
        assert!(!response.message.contains(secret));
        assert!(response.message.contains("[REDACTED]"));
    }

    #[test]
    fn debug_never_prints_api_key() {
        let provider = OpenAiResponsesProvider::with_http(
            OpenAiResponsesConfig::luna_preset(),
            Some("sk-super-secret-value".into()),
            Arc::new(FixedHttp::new(200, "{}", None)),
            OPENAI_LUNA_PROVIDER_ID,
        );
        let dbg = format!("{provider:?}");
        assert!(!dbg.contains("sk-super-secret-value"));
        assert!(dbg.contains("<redacted>"));
        assert!(dbg.contains("gpt-6-luna"));
        assert!(dbg.contains("openai-luna"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_credential_is_authentication_required() {
        let provider = luna_provider(Arc::new(FixedHttp::new(200, "{}", None)), None);
        let err = provider.respond(sample_request("hi")).await;
        match err.expect_err("missing key must fail") {
            AgentError::AuthenticationRequired { provider, message } => {
                assert_eq!(provider, "openai-luna");
                assert!(message.contains("OPENAI_API_KEY"));
            }
            other => panic!("expected AuthenticationRequired, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_auth_rate_limit_unavailable_timeout() {
        let p = luna_provider(
            Arc::new(FixedHttp::new(
                401,
                r#"{"error":{"message":"bad key"}}"#,
                None,
            )),
            Some("k"),
        );
        match p.respond(sample_request("x")).await.unwrap_err() {
            AgentError::AuthenticationRequired { .. } => {}
            other => panic!("{other:?}"),
        }

        let p = luna_provider(
            Arc::new(FixedHttp::new(
                429,
                r#"{"error":{"message":"slow down"}}"#,
                Some(7),
            )),
            Some("k"),
        );
        match p.respond(sample_request("x")).await.unwrap_err() {
            AgentError::RateLimited {
                retry_after_secs: Some(7),
                ..
            } => {}
            other => panic!("{other:?}"),
        }

        let p = luna_provider(
            Arc::new(FixedHttp::new(
                404,
                r#"{"error":{"message":"The model `gpt-6-luna` does not exist"}}"#,
                None,
            )),
            Some("k"),
        );
        match p.respond(sample_request("x")).await.unwrap_err() {
            AgentError::ProviderUnavailable { .. } => {}
            other => panic!("{other:?}"),
        }

        struct TimeoutHttp;
        impl HttpPost for TimeoutHttp {
            fn post_json(
                &self,
                _: &str,
                _: &[(String, String)],
                _: &str,
            ) -> Result<HttpResponseSnapshot, String> {
                Err("timeout: deadline exceeded".into())
            }
        }
        let p = luna_provider(Arc::new(TimeoutHttp), Some("k"));
        match p.respond(sample_request("x")).await.unwrap_err() {
            AgentError::Timeout(_) => {}
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_success_and_malformed_action() {
        let p = luna_provider(
            Arc::new(FixedHttp::new(200, &ok_body("hello", "explanation"), None)),
            Some("k"),
        );
        let resp = p.respond(sample_request("x")).await.unwrap();
        assert_eq!(resp.kind, AgentResponseKind::Explanation);
        assert_eq!(resp.message, "hello");
        assert_eq!(resp.references, vec!["@last".to_string()]);
        assert_eq!(resp.proposed_actions.len(), 0);

        let bad = json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": r#"{"kind":"proposal","message":"do it","proposed_actions":[{"action_type":"execute_command"}],"references":[],"uncertainty":null}"#
                }]
            }]
        })
        .to_string();
        let p = luna_provider(Arc::new(FixedHttp::new(200, &bad, None)), Some("k"));
        match p.respond(sample_request("x")).await.unwrap_err() {
            AgentError::Rejected(msg) => assert!(msg.contains("malformed") || msg.contains("argv")),
            other => panic!("{other:?}"),
        }

        // Authority: kind=explanation must not bypass action validation.
        let malformed_explanation = json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": r#"{"kind":"explanation","message":"safe text","proposed_actions":[{"action_type":"execute_command","argv":["ok",3]}],"references":[],"uncertainty":null}"#
                }]
            }]
        })
        .to_string();
        let p = luna_provider(
            Arc::new(FixedHttp::new(200, &malformed_explanation, None)),
            Some("k"),
        );
        assert!(matches!(
            p.respond(sample_request("x")).await.unwrap_err(),
            AgentError::Rejected(_)
        ));

        let p = luna_provider(Arc::new(FixedHttp::new(200, "not-json", None)), Some("k"));
        match p.respond(sample_request("x")).await.unwrap_err() {
            AgentError::Rejected(msg) => assert!(msg.contains("malformed structured response")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn successful_action_mapping_roundtrip() {
        let ok = json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": json!({
                        "kind": "proposal",
                        "message": "try cargo check",
                        "proposed_actions": [{
                            "action_type": "execute_tool",
                            "path": null,
                            "tool": "cargo",
                            "operation": "check",
                            "args": [],
                            "cwd": null,
                            "argv": [],
                            "action": null
                        }],
                        "references": [],
                        "uncertainty": "if workspace is dirty"
                    }).to_string()
                }]
            }]
        })
        .to_string();
        let p = luna_provider(Arc::new(FixedHttp::new(200, &ok, None)), Some("k"));
        let resp = p.respond(sample_request("x")).await.unwrap();
        assert_eq!(resp.kind, AgentResponseKind::Proposal);
        assert_eq!(resp.proposed_actions.len(), 1);
        assert!(matches!(
            &resp.proposed_actions[0],
            ProposedAction::ExecuteTool { tool, operation, .. }
                if tool == "cargo" && operation == "check"
        ));
        assert_eq!(resp.uncertainty.as_deref(), Some("if workspace is dirty"));
    }

    #[test]
    fn completed_valid_structured_output_passes() {
        let resp =
            OpenAiResponsesProvider::parse_responses_body(&ok_body("done", "result")).unwrap();
        assert_eq!(resp.kind, AgentResponseKind::Result);
        assert_eq!(resp.message, "done");
    }

    #[test]
    fn incomplete_max_output_tokens_fails_closed_with_reason() {
        let body = envelope_with_status(
            "incomplete",
            json!({ "incomplete_details": { "reason": "max_output_tokens" } }),
        );
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("status=incomplete"));
        assert!(err.message.contains("reason=max_output_tokens"));
        let agent = err.to_agent_error("openai-luna");
        assert!(matches!(agent, AgentError::Rejected(_)));
        let text = agent.to_string();
        assert!(text.contains("openai-luna"));
        assert!(text.contains("max_output_tokens"));
        assert!(!text.contains("malformed structured response"));
    }

    #[test]
    fn incomplete_content_filter_fails_closed_with_reason() {
        let body = envelope_with_status(
            "incomplete",
            json!({ "incomplete_details": { "reason": "content_filter" } }),
        );
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("reason=content_filter"));
    }

    #[test]
    fn incomplete_unknown_reason_is_preserved() {
        let body = envelope_with_status(
            "incomplete",
            json!({ "incomplete_details": { "reason": "some_future_reason" } }),
        );
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert!(err.message.contains("reason=some_future_reason"));
    }

    #[test]
    fn incomplete_without_reason_fails_closed() {
        let body = envelope_with_status("incomplete", json!({}));
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("status=incomplete"));
        assert!(err.message.contains("reason=unknown"));
    }

    #[test]
    fn failed_with_error_object_fails_closed_with_detail() {
        let body = envelope_with_status(
            "failed",
            json!({ "error": { "message": "upstream model exploded safely" } }),
        );
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("status=failed"));
        assert!(err.message.contains("upstream model exploded safely"));
    }

    #[test]
    fn cancelled_fails_closed_truthfully() {
        let body = envelope_with_status("cancelled", json!({}));
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("status=cancelled"));
        assert!(!err.message.to_lowercase().contains("timeout"));
        assert!(!err.message.to_lowercase().contains("auth"));
    }

    #[test]
    fn queued_and_in_progress_fail_closed() {
        for status in ["queued", "in_progress"] {
            let body = envelope_with_status(status, json!({}));
            let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
            assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
            assert!(err.message.contains(status));
            assert!(err.message.contains("unexpected"));
        }
    }

    #[test]
    fn missing_status_fails_closed() {
        let body = r#"{"output":[]}"#;
        let err = OpenAiResponsesProvider::parse_responses_body(body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("missing response status"));
    }

    #[test]
    fn unknown_status_string_fails_closed() {
        let body = envelope_with_status("future_status_xyz", json!({}));
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("future_status_xyz"));
    }

    #[test]
    fn non_completed_status_never_parses_partial_output() {
        // Even with completed-looking structured text, incomplete must not parse.
        let body = json!({
            "status": "incomplete",
            "incomplete_details": { "reason": "max_output_tokens" },
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": json!({
                        "kind": "explanation",
                        "message": "partial",
                        "proposed_actions": [],
                        "references": [],
                        "uncertainty": null
                    }).to_string()
                }]
            }]
        })
        .to_string();
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::IncompleteResponse);
        assert!(err.message.contains("max_output_tokens"));
    }

    #[test]
    fn completed_malformed_payload_still_rejected() {
        let body = json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "not-json" }]
            }]
        })
        .to_string();
        let err = OpenAiResponsesProvider::parse_responses_body(&body).unwrap_err();
        assert_eq!(err.class, OpenAiFailureClass::MalformedResponse);
        let agent = err.to_agent_error("openai-luna");
        assert!(matches!(agent, AgentError::Rejected(_)));
        assert!(agent.to_string().contains("malformed structured response"));
    }

    #[test]
    fn descriptor_never_contains_secret() {
        let desc = openai_luna_descriptor("gpt-6-luna", true);
        assert_eq!(desc.id, "openai-luna");
        assert_eq!(
            desc.credential_source.as_deref(),
            Some("environment:OPENAI_API_KEY")
        );
        assert!(desc.is_available);
        let s = format!("{desc:?}");
        assert!(!s.contains("Bearer"));
        assert!(!s.to_lowercase().contains("sk-"));
    }

    #[test]
    fn http_error_classification_is_distinct() {
        let rate = classify_http(429, "{}", Some(3)).unwrap_err();
        assert_eq!(rate.class, OpenAiFailureClass::RateLimited);

        let auth = classify_http(401, "{}", None).unwrap_err();
        assert_eq!(auth.class, OpenAiFailureClass::AuthFailure);

        let forbidden = classify_http(403, r#"{"error":{"message":"does not have access"}}"#, None)
            .unwrap_err();
        assert_eq!(forbidden.class, OpenAiFailureClass::AccountNotEntitled);

        let model = classify_http(
            400,
            r#"{"error":{"message":"The model `gpt-6-luna` does not exist"}}"#,
            None,
        )
        .unwrap_err();
        assert_eq!(model.class, OpenAiFailureClass::ModelNotFound);

        let endpoint = classify_http(404, "{}", None).unwrap_err();
        assert_eq!(endpoint.class, OpenAiFailureClass::EndpointUnsupported);

        let provider = classify_http(500, "{}", None).unwrap_err();
        assert_eq!(provider.class, OpenAiFailureClass::ProviderFailure);

        assert!(classify_http(200, "{}", None).is_ok());
    }

    #[test]
    fn timeouts_are_not_provider_not_found() {
        let fail = OpenAiFailure {
            class: OpenAiFailureClass::Timeout,
            message: "timeout".into(),
            retry_after_secs: Some(30),
        };
        let err = fail.to_agent_error("openai-luna");
        assert!(matches!(err, AgentError::Timeout(_)));
        let text = err.to_string();
        assert!(!text.contains("not found"));
        assert!(!text.contains("empty"));
    }
}
