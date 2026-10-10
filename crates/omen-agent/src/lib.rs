//! Omen Agent Interoperability library.
//!
//! Provides canonical structured AgentContext, provider-neutral AgentProvider trait,
//! and built-in DiagnosticAgentProvider for human shell reasoning.

pub mod adapter_provider;
pub mod adapter_spawn;
pub mod anthropic;
pub mod canary;
pub mod codex;
pub mod conformance;
pub mod context;
pub mod credentials;
pub mod deterministic;
pub mod diagnostic_provider;
pub mod live_gate;
pub mod openai_responses;
pub mod provider;
pub mod registry;

pub use adapter_provider::*;
pub use adapter_spawn::*;
pub use anthropic::*;
pub use canary::*;
pub use codex::*;
pub use conformance::*;
pub use context::*;
pub use credentials::*;
pub use deterministic::*;
pub use diagnostic_provider::*;
pub use live_gate::{
    HERMETIC_REFUSED_MESSAGE, HERMETIC_TESTS_ENV_VAR, LIVE_REFUSED_MESSAGE, LIVE_TESTS_ENV_VAR,
    admit_for_explicit_live_test, admit_production, hermetic_denial_active, is_admitted,
};
pub use openai_responses::*;
pub use provider::*;
pub use registry::*;
