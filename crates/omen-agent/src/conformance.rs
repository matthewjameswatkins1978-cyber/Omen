//! Provider conformance harness: the executable form of Omen's provider contract.
//!
//! A [`ConformanceScenario`] names one semantic expectation Omen places on
//! every [`AgentProvider`]. [`check_scenario`] asserts the Omen-side shape of
//! the outcome (typed success kind vs. distinguished [`AgentError`]) without
//! knowing provider internals. Providers opt in per scenario: the harness
//! never forces a provider to manufacture a capability it does not claim.
//!
//! Scenario vocabulary lives here so future providers (Anthropic, Gemini,
//! local models, …) run the same battery instead of reimplementing it.
//! These names are semantic test vocabulary, not a plugin framework.
//!
//! Omen owns request/response/authority/failure semantics. The harness proves
//! providers stay behind the dashboard: they reason and propose, they never
//! execute, and their failures stay distinguishable.

use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind, ProposedAction,
};

/// One semantic expectation in the provider conformance battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConformanceScenario {
    SuccessExplanation,
    SuccessProposal,
    SuccessActionRequest,
    SuccessQuestion,
    SuccessRefusal,
    AuthMissing,
    AuthRejected,
    ProviderUnavailable,
    ModelUnavailable,
    RateLimited,
    Timeout,
    MalformedResponse,
    UnsupportedCapability,
    TransportFailure,
    DisconnectBeforeResponse,
    ResponseTooLarge,
    TooManyActions,
    MalformedAction,
    UnknownResponseKind,
    IncompleteProviderResponse,
    ProviderRecoversAfterFailure,
}

impl ConformanceScenario {
    /// Stable machine-readable name for reports.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SuccessExplanation => "SUCCESS_EXPLANATION",
            Self::SuccessProposal => "SUCCESS_PROPOSAL",
            Self::SuccessActionRequest => "SUCCESS_ACTION_REQUEST",
            Self::SuccessQuestion => "SUCCESS_QUESTION",
            Self::SuccessRefusal => "SUCCESS_REFUSAL",
            Self::AuthMissing => "AUTH_MISSING",
            Self::AuthRejected => "AUTH_REJECTED",
            Self::ProviderUnavailable => "PROVIDER_UNAVAILABLE",
            Self::ModelUnavailable => "MODEL_UNAVAILABLE",
            Self::RateLimited => "RATE_LIMITED",
            Self::Timeout => "TIMEOUT",
            Self::MalformedResponse => "MALFORMED_RESPONSE",
            Self::UnsupportedCapability => "UNSUPPORTED_CAPABILITY",
            Self::TransportFailure => "TRANSPORT_FAILURE",
            Self::DisconnectBeforeResponse => "DISCONNECT_BEFORE_RESPONSE",
            Self::ResponseTooLarge => "RESPONSE_TOO_LARGE",
            Self::TooManyActions => "TOO_MANY_ACTIONS",
            Self::MalformedAction => "MALFORMED_ACTION",
            Self::UnknownResponseKind => "UNKNOWN_RESPONSE_KIND",
            Self::IncompleteProviderResponse => "INCOMPLETE_PROVIDER_RESPONSE",
            Self::ProviderRecoversAfterFailure => "PROVIDER_RECOVERS_AFTER_FAILURE",
        }
    }

    /// Every scenario in the battery.
    pub fn all() -> Vec<Self> {
        vec![
            Self::SuccessExplanation,
            Self::SuccessProposal,
            Self::SuccessActionRequest,
            Self::SuccessQuestion,
            Self::SuccessRefusal,
            Self::AuthMissing,
            Self::AuthRejected,
            Self::ProviderUnavailable,
            Self::ModelUnavailable,
            Self::RateLimited,
            Self::Timeout,
            Self::MalformedResponse,
            Self::UnsupportedCapability,
            Self::TransportFailure,
            Self::DisconnectBeforeResponse,
            Self::ResponseTooLarge,
            Self::TooManyActions,
            Self::MalformedAction,
            Self::UnknownResponseKind,
            Self::IncompleteProviderResponse,
            Self::ProviderRecoversAfterFailure,
        ]
    }
}

/// Outcome of one conformance probe.
#[derive(Debug, Clone)]
pub struct ProviderProbeResult {
    pub scenario: ConformanceScenario,
    pub passed: bool,
    pub detail: String,
}

/// Structural validity of one already-typed proposal.
///
/// This checks shape only (required fields present, non-empty where the
/// variant demands it, generous length sanity). Byte-level admission bounds
/// remain adapter-enforced (e.g. the Responses transport caps fields and
/// action counts); the harness proves Omen handles the typed outcome, it
/// does not re-implement adapter parsing.
pub fn validate_proposed_action(action: &ProposedAction) -> Result<(), String> {
    const MAX_FIELD_CHARS: usize = 8_000;
    const MAX_LIST_ITEMS: usize = 64;
    fn bounded(s: &str, what: &str) -> Result<(), String> {
        if s.chars().count() > MAX_FIELD_CHARS {
            return Err(format!("{what} exceeds {MAX_FIELD_CHARS} chars"));
        }
        Ok(())
    }
    fn bounded_list(items: &[String], what: &str) -> Result<(), String> {
        if items.len() > MAX_LIST_ITEMS {
            return Err(format!("{what} exceeds {MAX_LIST_ITEMS} items"));
        }
        for item in items {
            bounded(item, what)?;
        }
        Ok(())
    }
    match action {
        ProposedAction::ChangeDirectory { path } => {
            if path.as_os_str().is_empty() {
                return Err("change_directory missing path".into());
            }
            bounded(&path.to_string_lossy(), "change_directory.path")
        }
        ProposedAction::ExecuteTool {
            tool,
            operation,
            args,
            cwd,
        } => {
            if tool.trim().is_empty() {
                return Err("execute_tool missing tool".into());
            }
            bounded(tool, "execute_tool.tool")?;
            bounded(operation, "execute_tool.operation")?;
            bounded_list(args, "execute_tool.args")?;
            if let Some(c) = cwd {
                bounded(c, "execute_tool.cwd")?;
            }
            Ok(())
        }
        ProposedAction::ExecuteCommand { argv, cwd } => {
            if argv.is_empty() {
                return Err("execute_command missing argv".into());
            }
            bounded_list(argv, "execute_command.argv")?;
            if let Some(c) = cwd {
                bounded(c, "execute_command.cwd")?;
            }
            Ok(())
        }
        ProposedAction::SemanticAction { action, args } => {
            if action.trim().is_empty() {
                return Err("semantic_action missing action".into());
            }
            bounded(action, "semantic_action.action")?;
            bounded_list(args, "semantic_action.args")?;
            Ok(())
        }
    }
}

fn require_ok(
    scenario: ConformanceScenario,
    result: Result<AgentResponse, AgentError>,
) -> Result<AgentResponse, String> {
    result.map_err(|e| {
        format!(
            "{}: expected typed success, got failure {e:?}",
            scenario.name()
        )
    })
}

fn require_actions_empty(
    scenario: ConformanceScenario,
    resp: &AgentResponse,
) -> Result<(), String> {
    if resp.proposed_actions.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}: kind {:?} must not smuggle proposals ({} present)",
            scenario.name(),
            resp.kind,
            resp.proposed_actions.len()
        ))
    }
}

fn require_actions_present(
    scenario: ConformanceScenario,
    resp: &AgentResponse,
) -> Result<(), String> {
    if resp.proposed_actions.is_empty() {
        return Err(format!(
            "{}: expected at least one proposed action",
            scenario.name()
        ));
    }
    for action in &resp.proposed_actions {
        validate_proposed_action(action).map_err(|e| {
            format!(
                "{}: proposed action failed shape validation: {e}",
                scenario.name()
            )
        })?;
    }
    Ok(())
}

fn require_message(scenario: ConformanceScenario, resp: &AgentResponse) -> Result<(), String> {
    if resp.message.trim().is_empty() {
        return Err(format!(
            "{}: success with an empty message is not a success",
            scenario.name()
        ));
    }
    Ok(())
}

/// Asserts the Omen-side shape of one provider outcome.
///
/// `provider` must already be configured for the scenario (canary mode,
/// mocked transport script, …). The harness never configures providers; it
/// only judges outcomes against Omen semantics:
///
/// - typed successes carry the expected [`AgentResponseKind`];
/// - refusal is a success, never a transport failure;
/// - failures arrive as the distinguished [`AgentError`] variant, never as
///   empty success, never as "not found".
pub fn check_scenario(
    scenario: ConformanceScenario,
    provider: &dyn AgentProvider,
    request: &AgentRequest,
) -> Result<String, String> {
    // Providers are synchronous-behind-async; block briefly (canary and mocks
    // never stall; Timeout scenarios wrap the provider in TimeoutProvider).
    let result = crate::context::block_on_async(provider.respond(request.clone()));
    match scenario {
        ConformanceScenario::SuccessExplanation => {
            let resp = require_ok(scenario, result)?;
            if resp.kind != AgentResponseKind::Explanation {
                return Err(format!(
                    "{}: expected Explanation, got {:?}",
                    scenario.name(),
                    resp.kind
                ));
            }
            require_message(scenario, &resp)?;
            require_actions_empty(scenario, &resp)?;
            Ok(format!("explanation ({} chars)", resp.message.len()))
        }
        ConformanceScenario::SuccessProposal => {
            let resp = require_ok(scenario, result)?;
            if resp.kind != AgentResponseKind::Proposal {
                return Err(format!(
                    "{}: expected Proposal, got {:?}",
                    scenario.name(),
                    resp.kind
                ));
            }
            require_message(scenario, &resp)?;
            require_actions_present(scenario, &resp)?;
            Ok(format!(
                "proposal with {} actions",
                resp.proposed_actions.len()
            ))
        }
        ConformanceScenario::SuccessActionRequest => {
            let resp = require_ok(scenario, result)?;
            if resp.kind != AgentResponseKind::ActionRequest {
                return Err(format!(
                    "{}: expected ActionRequest, got {:?}",
                    scenario.name(),
                    resp.kind
                ));
            }
            require_message(scenario, &resp)?;
            require_actions_present(scenario, &resp)?;
            Ok(format!(
                "action request with {} actions",
                resp.proposed_actions.len()
            ))
        }
        ConformanceScenario::SuccessQuestion => {
            let resp = require_ok(scenario, result)?;
            if resp.kind != AgentResponseKind::Question {
                return Err(format!(
                    "{}: expected Question, got {:?}",
                    scenario.name(),
                    resp.kind
                ));
            }
            require_message(scenario, &resp)?;
            require_actions_empty(scenario, &resp)?;
            Ok("question".into())
        }
        ConformanceScenario::SuccessRefusal => {
            let resp = require_ok(scenario, result)?;
            if resp.kind != AgentResponseKind::Refusal {
                return Err(format!(
                    "{}: expected Refusal, got {:?}",
                    scenario.name(),
                    resp.kind
                ));
            }
            require_message(scenario, &resp)?;
            require_actions_empty(scenario, &resp)?;
            Ok("refusal (typed success, not failure)".into())
        }
        ConformanceScenario::AuthMissing | ConformanceScenario::AuthRejected => match result {
            Err(AgentError::AuthenticationRequired { provider, .. }) => {
                Ok(format!("authentication required by {provider}"))
            }
            Err(other) => Err(format!(
                "{}: expected AuthenticationRequired, got {other:?}",
                scenario.name()
            )),
            Ok(resp) => Err(format!(
                "{}: missing/rejected credential must never succeed (kind {:?})",
                scenario.name(),
                resp.kind
            )),
        },
        ConformanceScenario::ProviderUnavailable | ConformanceScenario::ModelUnavailable => {
            match result {
                Err(AgentError::ProviderUnavailable { provider, .. }) => {
                    Ok(format!("unavailable reported by {provider}"))
                }
                Err(other) => Err(format!(
                    "{}: expected ProviderUnavailable, got {other:?}",
                    scenario.name()
                )),
                Ok(resp) => Err(format!(
                    "{}: unavailable provider must never succeed (kind {:?})",
                    scenario.name(),
                    resp.kind
                )),
            }
        }
        ConformanceScenario::RateLimited => match result {
            Err(AgentError::RateLimited { .. }) => Ok("rate limited (retry is explicit)".into()),
            Err(other) => Err(format!(
                "{}: expected RateLimited, got {other:?}",
                scenario.name()
            )),
            Ok(resp) => Err(format!(
                "{}: rate-limited call must never succeed (kind {:?})",
                scenario.name(),
                resp.kind
            )),
        },
        ConformanceScenario::Timeout => match result {
            Err(AgentError::Timeout(d)) => Ok(format!("timeout after {d:?}, state unchanged")),
            Err(other) => Err(format!(
                "{}: expected Timeout, got {other:?}",
                scenario.name()
            )),
            Ok(resp) => Err(format!(
                "{}: timed-out call must never succeed (kind {:?})",
                scenario.name(),
                resp.kind
            )),
        },
        ConformanceScenario::MalformedResponse
        | ConformanceScenario::ResponseTooLarge
        | ConformanceScenario::TooManyActions
        | ConformanceScenario::MalformedAction
        | ConformanceScenario::UnknownResponseKind
        | ConformanceScenario::IncompleteProviderResponse => match result {
            Err(AgentError::Rejected(msg)) => Ok(format!("rejected: {msg}")),
            Err(other) => Err(format!(
                "{}: expected Rejected, got {other:?}",
                scenario.name()
            )),
            Ok(resp) => Err(format!(
                "{}: malformed output must never be admitted (kind {:?})",
                scenario.name(),
                resp.kind
            )),
        },
        ConformanceScenario::UnsupportedCapability => match result {
            Err(AgentError::UnsupportedCapability {
                provider,
                capability,
            }) => Ok(format!("{provider} lacks {capability}")),
            Err(other) => Err(format!(
                "{}: expected UnsupportedCapability, got {other:?}",
                scenario.name()
            )),
            Ok(resp) => Err(format!(
                "{}: unsupported capability must fail (kind {:?})",
                scenario.name(),
                resp.kind
            )),
        },
        ConformanceScenario::TransportFailure | ConformanceScenario::DisconnectBeforeResponse => {
            match result {
                Err(AgentError::Provider(msg)) => Ok(format!("transport truth: {msg}")),
                Err(other) => Err(format!(
                    "{}: expected Provider transport error, got {other:?}",
                    scenario.name()
                )),
                Ok(resp) => Err(format!(
                    "{}: failed transport must never invent success (kind {:?})",
                    scenario.name(),
                    resp.kind
                )),
            }
        }
        ConformanceScenario::ProviderRecoversAfterFailure => {
            // Recovery is a two-call sequence: the caller scripts
            // failure-then-success; the harness judges both halves.
            let first = result;
            if first.is_ok() {
                return Err(format!(
                    "{}: first call must fail (scripted failure)",
                    scenario.name()
                ));
            }
            let second = crate::context::block_on_async(provider.respond(request.clone()));
            match second {
                Ok(resp) => {
                    require_message(scenario, &resp)?;
                    Ok(format!(
                        "recovered: first failed, second returned {:?}",
                        resp.kind
                    ))
                }
                Err(e) => Err(format!(
                    "{}: provider did not recover, second call failed: {e:?}",
                    scenario.name()
                )),
            }
        }
    }
}

/// Runs a scenario battery against one configured provider.
pub fn run_conformance(
    provider: &dyn AgentProvider,
    request: &AgentRequest,
    scenarios: &[ConformanceScenario],
) -> Vec<ProviderProbeResult> {
    scenarios
        .iter()
        .map(|s| match check_scenario(*s, provider, request) {
            Ok(detail) => ProviderProbeResult {
                scenario: *s,
                passed: true,
                detail,
            },
            Err(detail) => ProviderProbeResult {
                scenario: *s,
                passed: false,
                detail,
            },
        })
        .collect()
}

/// One canonical capability in Omen's provider vocabulary.
///
/// Descriptors still serialize `capabilities` as plain strings (stable
/// machine output), but every emitted string must be one of these spellings.
/// Provider-branded capabilities (`luna-fast`, `openai-json`, …) are banned:
/// capabilities describe what Omen may ask for, never which vendor answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderCapability {
    Reasoning,
    StructuredResponse,
    FailureDiagnosis,
    Navigation,
    Proposal,
    ToolProposal,
    Deterministic,
    Orientation,
    BuildCheck,
}

impl ProviderCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::StructuredResponse => "structured-response",
            Self::FailureDiagnosis => "failure-diagnosis",
            Self::Navigation => "navigation",
            Self::Proposal => "proposal",
            Self::ToolProposal => "tool-proposal",
            Self::Deterministic => "deterministic",
            Self::Orientation => "orientation",
            Self::BuildCheck => "build-check",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reasoning" => Some(Self::Reasoning),
            "structured-response" => Some(Self::StructuredResponse),
            "failure-diagnosis" => Some(Self::FailureDiagnosis),
            "navigation" => Some(Self::Navigation),
            "proposal" => Some(Self::Proposal),
            "tool-proposal" => Some(Self::ToolProposal),
            "deterministic" => Some(Self::Deterministic),
            "orientation" => Some(Self::Orientation),
            "build-check" => Some(Self::BuildCheck),
            _ => None,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Reasoning,
            Self::StructuredResponse,
            Self::FailureDiagnosis,
            Self::Navigation,
            Self::Proposal,
            Self::ToolProposal,
            Self::Deterministic,
            Self::Orientation,
            Self::BuildCheck,
        ]
    }
}

/// True when `s` is a canonical capability spelling.
pub fn is_canonical_capability(s: &str) -> bool {
    ProviderCapability::parse(s).is_some()
}
