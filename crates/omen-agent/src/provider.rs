use crate::context::AgentContext;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_AGENT_TIMEOUT: Duration = Duration::from_secs(30);

/// A single turn in an ongoing Agent conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTurn {
    pub role: String,
    pub text: String,
    pub timestamp: String,
}

/// Typed intent and actions proposed by an Agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action_type", rename_all = "snake_case")]
pub enum ProposedAction {
    /// Navigate session CWD to a different directory.
    ChangeDirectory { path: PathBuf },
    /// Execute a typed tool operation through Omen's execution engine or daemon broker.
    ExecuteTool {
        tool: String,
        operation: String,
        args: Vec<String>,
        cwd: Option<String>,
    },
    /// Execute an arbitrary argv command via ProcessSupervisor.
    ExecuteCommand {
        argv: Vec<String>,
        cwd: Option<String>,
    },
    /// Perform a deterministic semantic action (:show, :inspect, :why, etc.).
    SemanticAction { action: String, args: Vec<String> },
}

/// Category of Agent response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentResponseKind {
    Explanation,
    Proposal,
    ActionRequest,
    Result,
    Question,
    Refusal,
}

/// User query and surrounding bounded Omen machine context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub prompt: String,
    pub context: AgentContext,
    pub conversation: Vec<AgentTurn>,
}

/// Structured response returned by an Agent provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentResponse {
    pub kind: AgentResponseKind,
    pub message: String,
    pub proposed_actions: Vec<ProposedAction>,
    pub references: Vec<String>,
    pub uncertainty: Option<String>,
}

impl AgentResponse {
    pub fn explanation(message: impl Into<String>) -> Self {
        Self {
            kind: AgentResponseKind::Explanation,
            message: message.into(),
            proposed_actions: Vec::new(),
            references: Vec::new(),
            uncertainty: None,
        }
    }

    pub fn proposal(message: impl Into<String>, actions: Vec<ProposedAction>) -> Self {
        Self {
            kind: AgentResponseKind::Proposal,
            message: message.into(),
            proposed_actions: actions,
            references: Vec::new(),
            uncertainty: None,
        }
    }

    pub fn question(message: impl Into<String>) -> Self {
        Self {
            kind: AgentResponseKind::Question,
            message: message.into(),
            proposed_actions: Vec::new(),
            references: Vec::new(),
            uncertainty: None,
        }
    }

    pub fn refusal(message: impl Into<String>) -> Self {
        Self {
            kind: AgentResponseKind::Refusal,
            message: message.into(),
            proposed_actions: Vec::new(),
            references: Vec::new(),
            uncertainty: None,
        }
    }
}

/// Errors occurring during Agent invocation, translated to stable Omen concepts.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AgentError {
    #[error("Authentication required for agent provider '{provider}': {message}")]
    AuthenticationRequired { provider: String, message: String },

    #[error("Agent provider '{provider}' unavailable: {message}")]
    ProviderUnavailable { provider: String, message: String },

    #[error("Agent provider '{provider}' rate limited. Retry after {retry_after_secs:?} seconds.")]
    RateLimited {
        provider: String,
        retry_after_secs: Option<u64>,
    },

    #[error("Agent request exceeded {0:?}. Omen state is unchanged.")]
    Timeout(Duration),

    #[error("Unsupported capability '{capability}' for agent provider '{provider}'.")]
    UnsupportedCapability {
        provider: String,
        capability: String,
    },

    #[error("Agent provider error: {0}")]
    Provider(String),

    #[error("Agent request rejected: {0}")]
    Rejected(String),
}

/// Provider-neutral interface for conversational and diagnostic Agents.
pub trait AgentProvider: Send + Sync {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>>;
}

/// Wraps any AgentProvider with an explicit execution ceiling to prevent hangs.
pub struct TimeoutProvider {
    inner: Arc<dyn AgentProvider>,
    timeout: Duration,
}

impl TimeoutProvider {
    pub fn new(inner: Arc<dyn AgentProvider>, timeout: Duration) -> Self {
        Self { inner, timeout }
    }
}

impl AgentProvider for TimeoutProvider {
    fn respond<'a>(
        &'a self,
        request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(async move {
            match tokio::time::timeout(self.timeout, self.inner.respond(request)).await {
                Ok(result) => result,
                Err(_) => Err(AgentError::Timeout(self.timeout)),
            }
        })
    }
}
