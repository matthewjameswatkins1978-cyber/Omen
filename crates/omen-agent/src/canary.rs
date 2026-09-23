//! Deterministic hostile provider: Omen's provider canary.
//!
//! [`ConformanceProvider`] implements [`AgentProvider`] with zero network,
//! zero API credits, and zero unbounded waits. Each [`ConformanceMode`]
//! deterministically produces one provider state on demand (success shapes,
//! auth failures, rate limits, transport failures, disconnects, malformed
//! output, oversized output, …) so the conformance harness can prove Omen
//! semantics without spending money.
//!
//! The canary screams when provider semantics drift: every mode has an exact
//! expected Omen-side outcome asserted by the harness.
//!
//! Instrumentation: every call increments an invocation counter and captures
//! the received [`AgentRequest`], so tests prove single invocation (no silent
//! retry), prompt pass-through, and credential hygiene.

use crate::provider::{
    AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind, ProposedAction,
};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const CANARY_PROVIDER_ID: &str = "conformance-canary";
const CANARY_MODEL: &str = "deterministic-canary";

/// One deterministic provider state the canary can produce.
#[derive(Debug, Clone)]
pub enum ConformanceMode {
    SuccessExplanation,
    SuccessProposal,
    SuccessActionRequest,
    SuccessQuestion,
    SuccessRefusal,
    /// Proposal carrying caller-supplied argv (authority-boundary proofs).
    ProposalExec(Vec<String>),
    AuthMissing,
    AuthRejected,
    ProviderUnavailable,
    ModelUnavailable,
    RateLimited,
    /// Immediate deterministic timeout error (no waiting).
    TimeoutNow,
    /// Sleep before succeeding (drives real [`TimeoutProvider`] ceilings).
    Sleep(Duration),
    Malformed,
    UnsupportedCapability,
    TransportFailure,
    Disconnect,
    ResponseTooLarge,
    TooManyActions,
    MalformedAction,
    UnknownKind,
    Incomplete,
    /// Per-call script; each invocation pops the front. Exhaustion is an
    /// explicit error — the canary never replays past its script.
    Script(VecDeque<ConformanceMode>),
}

/// Deterministic hostile [`AgentProvider`] fixture.
pub struct ConformanceProvider {
    mode: Mutex<ConformanceMode>,
    calls: AtomicU64,
    last_request: Mutex<Option<AgentRequest>>,
}

impl ConformanceProvider {
    pub fn new(mode: ConformanceMode) -> Self {
        Self {
            mode: Mutex::new(mode),
            calls: AtomicU64::new(0),
            last_request: Mutex::new(None),
        }
    }

    /// Replace the current mode (single-state scenarios).
    pub fn set_mode(&self, mode: ConformanceMode) {
        *self.mode.lock().unwrap() = mode;
    }

    /// Total invocations so far. Retry proofs assert exact counts.
    pub fn call_count(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    /// The most recently received request (prompt/context pass-through proofs).
    pub fn last_request(&self) -> Option<AgentRequest> {
        self.last_request.lock().unwrap().clone()
    }

    pub fn provider_id(&self) -> &'static str {
        CANARY_PROVIDER_ID
    }

    pub fn model(&self) -> &'static str {
        CANARY_MODEL
    }

    fn next_mode(&self) -> Result<ConformanceMode, AgentError> {
        let mut guard = self.mode.lock().unwrap();
        match &mut *guard {
            ConformanceMode::Script(queue) => queue.pop_front().ok_or_else(|| {
                AgentError::Provider(
                    "canary script exhausted: no silent replay past the script".into(),
                )
            }),
            single => Ok(single.clone()),
        }
    }

    fn success_explanation() -> AgentResponse {
        AgentResponse {
            kind: AgentResponseKind::Explanation,
            message: "canary explanation: exit code 2 comes from the child process, not Omen"
                .into(),
            proposed_actions: Vec::new(),
            references: Vec::new(),
            uncertainty: None,
        }
    }

    fn success_proposal(argv: Vec<String>) -> AgentResponse {
        AgentResponse {
            kind: AgentResponseKind::Proposal,
            message: "canary proposes a typed command (proposal only, never executed)".into(),
            proposed_actions: vec![
                ProposedAction::ExecuteCommand { argv, cwd: None },
                ProposedAction::SemanticAction {
                    action: "show".into(),
                    args: vec!["@last".into()],
                },
            ],
            references: Vec::new(),
            uncertainty: None,
        }
    }

    fn success_action_request() -> AgentResponse {
        AgentResponse {
            kind: AgentResponseKind::ActionRequest,
            message: "canary requests a typed tool operation".into(),
            proposed_actions: vec![ProposedAction::ExecuteTool {
                tool: "cargo".into(),
                operation: "check".into(),
                args: Vec::new(),
                cwd: None,
            }],
            references: Vec::new(),
            uncertainty: None,
        }
    }

    fn success_question() -> AgentResponse {
        AgentResponse {
            kind: AgentResponseKind::Question,
            message: "canary question: which directory did you mean?".into(),
            proposed_actions: Vec::new(),
            references: Vec::new(),
            uncertainty: Some("canary is deliberately uncertain".into()),
        }
    }

    fn success_refusal() -> AgentResponse {
        AgentResponse {
            kind: AgentResponseKind::Refusal,
            message: "canary refuses: request is outside the canary contract".into(),
            proposed_actions: Vec::new(),
            references: Vec::new(),
            uncertainty: None,
        }
    }

    fn failure(mode: &ConformanceMode) -> AgentError {
        match mode {
            ConformanceMode::AuthMissing => AgentError::AuthenticationRequired {
                provider: CANARY_PROVIDER_ID.into(),
                message: "canary credential not configured".into(),
            },
            ConformanceMode::AuthRejected => AgentError::AuthenticationRequired {
                provider: CANARY_PROVIDER_ID.into(),
                message: "canary credential rejected".into(),
            },
            ConformanceMode::ProviderUnavailable => AgentError::ProviderUnavailable {
                provider: CANARY_PROVIDER_ID.into(),
                message: "canary provider down for maintenance".into(),
            },
            ConformanceMode::ModelUnavailable => AgentError::ProviderUnavailable {
                provider: CANARY_PROVIDER_ID.into(),
                message: "canary model withdrawn".into(),
            },
            ConformanceMode::RateLimited => AgentError::RateLimited {
                provider: CANARY_PROVIDER_ID.into(),
                retry_after_secs: Some(60),
            },
            ConformanceMode::TimeoutNow => AgentError::Timeout(Duration::from_secs(30)),
            ConformanceMode::Malformed => AgentError::Rejected(
                "canary returned a malformed structured response: envelope decode failed".into(),
            ),
            ConformanceMode::UnsupportedCapability => AgentError::UnsupportedCapability {
                provider: CANARY_PROVIDER_ID.into(),
                capability: "canary-unsupported-thing".into(),
            },
            ConformanceMode::TransportFailure => {
                AgentError::Provider("canary transport failed: connection reset".into())
            }
            ConformanceMode::Disconnect => {
                AgentError::Provider("canary disconnected before responding".into())
            }
            ConformanceMode::ResponseTooLarge => AgentError::Rejected(
                "canary structured response exceeded field bounds: message".into(),
            ),
            ConformanceMode::TooManyActions => {
                AgentError::Rejected("canary too many proposed actions (maximum 32)".into())
            }
            ConformanceMode::MalformedAction => AgentError::Rejected(
                "canary malformed proposed actions: execute_command missing argv".into(),
            ),
            ConformanceMode::UnknownKind => {
                AgentError::Rejected("canary unknown response kind 'telepathy'".into())
            }
            ConformanceMode::Incomplete => AgentError::Rejected(
                "canary response not completed: status=incomplete reason=max_output_tokens".into(),
            ),
            other => AgentError::Provider(format!("canary exhausted or invalid mode: {other:?}")),
        }
    }
}

impl AgentProvider for ConformanceProvider {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_request.lock().unwrap() = Some(request);
        let mode = self.next_mode();
        Box::pin(async move {
            let mode = match mode {
                Ok(m) => m,
                Err(e) => return Err(e),
            };
            match mode {
                ConformanceMode::SuccessExplanation => Ok(Self::success_explanation()),
                ConformanceMode::SuccessProposal => Ok(Self::success_proposal(vec![
                    "echo".into(),
                    "canary-proposal".into(),
                ])),
                ConformanceMode::ProposalExec(argv) => Ok(Self::success_proposal(argv)),
                ConformanceMode::SuccessActionRequest => Ok(Self::success_action_request()),
                ConformanceMode::SuccessQuestion => Ok(Self::success_question()),
                ConformanceMode::SuccessRefusal => Ok(Self::success_refusal()),
                ConformanceMode::Sleep(dur) => {
                    tokio::time::sleep(dur).await;
                    Ok(Self::success_explanation())
                }
                ConformanceMode::Script(_) => Err(AgentError::Provider(
                    "canary script exhausted: no silent replay past the script".into(),
                )),
                failure => Err(Self::failure(&failure)),
            }
        })
    }
}
