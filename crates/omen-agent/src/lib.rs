//! Omen Agent Interoperability library.
//!
//! Provides canonical structured AgentContext, provider-neutral AgentProvider trait,
//! and built-in DiagnosticAgentProvider for human shell reasoning.

pub mod context;
pub mod deterministic;
pub mod diagnostic_provider;
pub mod openai_luna;
pub mod provider;
pub mod registry;

pub use context::*;
pub use deterministic::*;
pub use diagnostic_provider::*;
pub use openai_luna::*;
pub use provider::*;
pub use registry::*;
