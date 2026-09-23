//! Exact binding identity. Answers which adapter, which bytes, which
//! protocol, which route — without recording secrets. Unknown model
//! identity is UNKNOWN (None), never invented.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A live binding among adapter bytes, protocol, transport, configuration,
/// and provider identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingIdentity {
    pub adapter_id: String,
    pub adapter_version: String,
    /// sha256 of the exact installed executable bytes that were spawned.
    pub adapter_digest: String,
    /// sha256 of the exact manifest bytes the binding was validated against.
    pub manifest_digest: String,
    pub protocol_version: String,
    pub provider_id: String,
    /// None means UNKNOWN — never invented when the harness hides it.
    pub model: Option<String>,
    pub transport: String,
    pub omen_contract: String,
    /// sha256 over sorted non-secret configuration (argv template, sandbox,
    /// flags, schema digest). No values, only shape.
    pub config_fingerprint: String,
    pub bound_at: String,
}

impl BindingIdentity {
    pub fn model_or_unknown(&self) -> &str {
        self.model.as_deref().unwrap_or("UNKNOWN")
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Fingerprint over sorted `key=value` shape pairs (no secret values).
pub fn fingerprint_pairs(pairs: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(&str, &str)> = pairs.to_vec();
    sorted.sort();
    let joined = sorted
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(joined.as_bytes())
}
