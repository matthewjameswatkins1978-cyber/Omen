//! Omen Agent Interoperability library.
//!
//! Provides canonical structured AgentContext, provider-neutral AgentProvider trait,
//! and built-in DiagnosticAgentProvider for human shell reasoning.

pub mod context;
pub mod diagnostic_provider;
pub mod provider;

pub use context::*;
pub use diagnostic_provider::*;
pub use provider::*;
