//! Exact Gate identity: trust is established, never inferred.
//!
//! Omen pins the expected product version, authority protocol, Tethers
//! source SHA, and Gate executable hash. The `hello` handshake must prove
//! protocol + product + features; the executable hash is verified before
//! spawn ([`crate::gate::GateProcess`]). Path alone proves nothing.

use crate::AuthorityError;
use crate::protocol::{AUTHORITY_PROTOCOL, HelloResult, TETHERS_PRODUCT_VERSION};

/// Canonical Tethers source SHA the managed companion is built from.
/// Single source of truth for the pin: CI checks out exactly this, the
/// E2E asserts the checkout matches, and release provenance records it.
pub const TETHERS_SOURCE_SHA: &str = "7e29110319c554a6586865ec6c47a45498696d16";

/// Pinned Gate identity (release provenance; handshake expectations).
#[derive(Debug, Clone)]
pub struct ExpectedGate {
    /// Exact authority protocol (`tethers.authority/1`).
    pub protocol: String,
    /// Exact compatible product version (`0.8.0`).
    pub product_version: String,
    /// Canonical Tethers source SHA the companion was built from.
    pub tethers_source_sha: String,
    /// SHA256 of the Gate executable (verified pre-spawn).
    pub gate_exe_sha256: Option<String>,
    /// Required feature names (subset check).
    pub features: Vec<String>,
}

impl ExpectedGate {
    pub fn pinned(tethers_source_sha: String, gate_exe_sha256: Option<String>) -> Self {
        Self {
            protocol: AUTHORITY_PROTOCOL.to_string(),
            product_version: TETHERS_PRODUCT_VERSION.to_string(),
            tethers_source_sha,
            gate_exe_sha256,
            features: vec![
                "prepare".to_string(),
                "approval_decision".to_string(),
                "commit".to_string(),
                "outcome".to_string(),
                "status".to_string(),
                "shutdown".to_string(),
            ],
        }
    }

    /// Verify a `hello` result against the pin. Any deviation refuses:
    /// wrong protocol, wrong product, missing features, or a Gate that
    /// claims standing authority (`authority_granted` must be false —
    /// Tethers proposes, Omen commits effects; nothing is pre-granted).
    pub fn verify_hello(&self, hello: &HelloResult) -> Result<(), AuthorityError> {
        if hello.protocol != self.protocol {
            return Err(AuthorityError::Validate(format!(
                "gate.protocol_mismatch: {} != {}",
                hello.protocol, self.protocol
            )));
        }
        if !hello.protocol_versions.contains(&self.protocol) {
            return Err(AuthorityError::Validate(
                "gate.protocol_not_advertised".to_string(),
            ));
        }
        if hello.product_version != self.product_version {
            return Err(AuthorityError::Validate(format!(
                "gate.product_mismatch: {} != {}",
                hello.product_version, self.product_version
            )));
        }
        for f in &self.features {
            if !hello.features.contains(f) {
                return Err(AuthorityError::Validate(format!(
                    "gate.feature_missing: {f}"
                )));
            }
        }
        if hello.authority_granted {
            return Err(AuthorityError::Validate(
                "gate.authority_premature: hello claims granted authority".to_string(),
            ));
        }
        Ok(())
    }
}
