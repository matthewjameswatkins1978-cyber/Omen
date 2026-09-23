//! Omen Agent Interoperability library.
//!
//! Provides canonical structured AgentContext, provider-neutral AgentProvider trait,
//! and built-in DiagnosticAgentProvider for human shell reasoning.

pub mod adapter_provider;
pub mod adapter_spawn;
pub mod canary;
pub mod codex;
pub mod conformance;
pub mod context;
pub mod deterministic;
pub mod diagnostic_provider;
pub mod openai_responses;
pub mod provider;
pub mod registry;

pub use adapter_provider::*;
pub use adapter_spawn::*;
pub use canary::*;
pub use codex::*;
pub use conformance::*;
pub use context::*;
pub use deterministic::*;
pub use diagnostic_provider::*;
pub use openai_responses::*;
pub use provider::*;
pub use registry::*;
