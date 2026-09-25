//! Canonical-JSON SHA256 digests: the exact `argument_digest` binding.
//!
//! Tethers computes `argument_digest` as `sha256:` + hex of the
//! canonical-JSON (JCS, sorted keys) serialisation of the Action
//! arguments. Omen recomputes the same digest over the exact arguments it
//! embedded in its intent and refuses to spawn on any mismatch. This is
//! pure arithmetic over Omen's own intent — no policy reimplemented.

use crate::AuthorityError;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// `sha256:<hex>` over canonical JSON of `value`.
pub fn canonical_digest(value: &Value) -> Result<String, AuthorityError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|e| AuthorityError::Validate(format!("digest.canonicalize.failed: {e}")))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// Verify a Gate-issued digest equals the digest of Omen's expected
/// arguments. Format-checked first; mismatch refuses with the binding
/// detail (never "best effort").
pub fn verify_argument_digest(
    issued: &str,
    expected_arguments: &Value,
) -> Result<(), AuthorityError> {
    if issued.len() != 7 + 64
        || !issued.starts_with("sha256:")
        || !issued[7..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(AuthorityError::Validate(format!(
            "dispatch.argument_digest_malformed: {issued}"
        )));
    }
    let recomputed = canonical_digest(expected_arguments)?;
    if issued != recomputed {
        return Err(AuthorityError::Validate(format!(
            "dispatch.argument_digest_mismatch: gate {issued} != intent {recomputed}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_is_key_order_independent() {
        let a = json!({"message": "LK-39", "path": "projects/r2-stdio"});
        let b = json!({"path": "projects/r2-stdio", "message": "LK-39"});
        assert_eq!(canonical_digest(&a).unwrap(), canonical_digest(&b).unwrap());
    }

    #[test]
    fn malformed_digest_refuses() {
        let args = json!({"a": 1});
        assert!(verify_argument_digest("nope", &args).is_err());
        assert!(verify_argument_digest("sha256:zzzz", &args).is_err());
    }
}
