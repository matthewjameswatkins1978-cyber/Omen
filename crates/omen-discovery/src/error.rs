//! Phase-named discovery errors.
//!
//! Standing rule 3: failures must name the phase. An agent should not have to
//! reconstruct the execution path from prose when Omen already knows it.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A bounded, phase-named discovery error.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[error("{phase}: {message}")]
pub struct DiscoveryError {
    /// Phase boundary, e.g. `discovery.provider.tool_options.harvest.timeout`.
    pub phase: String,
    pub message: String,
}

impl DiscoveryError {
    pub fn new(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
        }
    }

    /// `discovery.discover.*`
    pub fn discover(message: impl Into<String>) -> Self {
        Self::new("discovery.discover", message)
    }

    /// `discovery.provider.<id>.<phase>`
    pub fn provider(provider: &str, phase: &str, message: impl Into<String>) -> Self {
        Self::new(format!("discovery.provider.{provider}.{phase}"), message)
    }

    /// `discovery.schedule.*`
    pub fn schedule(message: impl Into<String>) -> Self {
        Self::new("discovery.schedule", message)
    }

    /// `discovery.cache.*`
    pub fn cache(message: impl Into<String>) -> Self {
        Self::new("discovery.cache", message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_the_phase() {
        let e = DiscoveryError::provider("tool_options", "harvest.timeout", "took too long");
        assert_eq!(e.phase, "discovery.provider.tool_options.harvest.timeout");
        assert!(e.to_string().contains("harvest.timeout"));
    }
}
