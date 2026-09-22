//! Host-side authority integration boundary (neutral data only).
//!
//! Doctrine: Tethers controls authority; Omen reports enforceability. Omen
//! is substrate, not sovereign. This module carries the SHAPES a live
//! Tethers Host seam would fill — identities, requests, verdicts — without
//! defining any permission, policy, approval, grant, token, or fallback
//! authority semantics. There is deliberately NO policy engine here.
//!
//! Current status: `BLOCKED_BY_TETHERS`. No stable external-Host admission
//! interface exists (no types, client, protocol, crate, URL, env, or server
//! anywhere in or around Omen; see `docs/evidence/0.9-E2/` assessment).
//! Therefore `check_live_admission` always returns `NotAdmitted` with
//! `ProviderUnavailable`: the fail-closed default. Absence of live authority
//! fails closed for operations documented as authority-required, and Omen
//! never infers authority from authentication, identity, model output,
//! previous approval, remembered state, or workspace metadata.
//!
//! When a live Tethers Host seam exists OUTSIDE Omen, that seam — not Omen —
//! supplies admission verdicts. Omen will never invent them.

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identity of a capability as named by the external authority (Tethers).
/// Omen transports this string opaquely; its meaning belongs to Tethers.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityIdentity(pub String);

/// Identity of an authority scope as named by the external authority.
/// Omen transports this string opaquely; its meaning belongs to Tethers.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeIdentity(pub String);

/// Reference to authority evidence held by the external authority
/// (e.g. a `tethers://...` URI). Omen stores and reports the reference; it
/// never interprets, mints, or validates the referenced authority itself.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthorityEvidenceReference(pub String);

macro_rules! define_authority_string {
    ($name:ident) => {
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, CoreError> {
                let s = value.into();
                if s.trim().is_empty() {
                    return Err(CoreError::InvalidId(format!(
                        "{} cannot be empty",
                        stringify!($name)
                    )));
                }
                if s.chars().any(|c| c.is_control()) {
                    return Err(CoreError::InvalidId(format!(
                        "{} contains control characters: {:?}",
                        stringify!($name),
                        s
                    )));
                }
                Ok(Self(s))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

define_authority_string!(CapabilityIdentity);
define_authority_string!(ScopeIdentity);
define_authority_string!(AuthorityEvidenceReference);

/// A request for current admission, phrased in the external authority's
/// own identity vocabulary. Data carrier only — Omen does not evaluate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRequest {
    pub capability: CapabilityIdentity,
    pub scope: ScopeIdentity,
    pub operation: String,
}

/// Why admission was not granted. These are the hostile cases E2 requires:
/// unavailable / missing / invalid / stale admission, wrong capability or
/// scope, and revocation. Carried as data; Omen manufactures none of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason")]
pub enum NotAdmittedReason {
    ProviderUnavailable,
    MissingAdmission,
    InvalidAdmission,
    StaleAdmission,
    WrongCapability,
    WrongScope,
    Revoked,
}

/// Verdict on an admission request. The `Admitted` variant exists so the
/// wire shape is stable for a future live seam; today only Omen-external
/// Tethers infrastructure may construct it — Omen itself never does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict")]
pub enum AdmissionVerdict {
    Admitted {
        evidence: AuthorityEvidenceReference,
    },
    NotAdmitted {
        reason: NotAdmittedReason,
    },
}

/// Current live-admission reality as Omen can truthfully report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum LiveAdmissionStatus {
    /// No stable external-Host admission interface exists. Evidence is a
    /// human-readable pointer to the assessment, not a policy.
    BlockedByTethers { evidence: String },
}

/// Report the current live-admission reality.
///
/// Today this is unconditionally `BlockedByTethers`: Omen has inspected its
/// surroundings and found no live Tethers Host seam to integrate against.
/// A visible blocker is acceptable; a fabricated semantic bridge is not.
pub fn live_admission_status() -> LiveAdmissionStatus {
    LiveAdmissionStatus::BlockedByTethers {
        evidence: "BLOCKED_BY_TETHERS: no Tethers admission types, client, protocol, crate, URL, env, or server exists in or around Omen; file-supplied ExecutionContract JSON is the only admission carrier (see docs/evidence/0.9-E2/TETHERS_SEAM_ASSESSMENT.md)".into(),
    }
}

/// Ask the live authority whether `request` is admitted right now.
///
/// Fail-closed default: with no live provider, EVERY request is
/// `NotAdmitted { ProviderUnavailable }` — regardless of how plausible the
/// capability, scope, identity, or operation looks. Omen infers nothing
/// from shape, authentication, history, or prior approval. Last-responsible-
/// moment checks belong immediately before consequential dispatch; when the
/// seam goes live outside Omen, that seam answers instead of this function.
pub fn check_live_admission(_request: &AdmissionRequest) -> AdmissionVerdict {
    AdmissionVerdict::NotAdmitted {
        reason: NotAdmittedReason::ProviderUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> AdmissionRequest {
        AdmissionRequest {
            capability: CapabilityIdentity::new("exec.run").unwrap(),
            scope: ScopeIdentity::new("workspace:default").unwrap(),
            operation: "dispatch".into(),
        }
    }

    #[test]
    fn live_admission_is_blocked_with_evidence() {
        match live_admission_status() {
            LiveAdmissionStatus::BlockedByTethers { evidence } => {
                assert!(evidence.contains("BLOCKED_BY_TETHERS"));
            }
        }
    }

    #[test]
    fn no_live_provider_means_no_admission_for_any_shape() {
        // Hostile shapes must not coax admission out of Omen: a plausible
        // capability, a previously-seen scope, an authenticated-looking
        // operation — all fail closed while no provider exists.
        for operation in [
            "dispatch",
            "admin",
            "previously-approved-step-1",
            "authenticated-agent-task",
        ] {
            let request = AdmissionRequest {
                capability: CapabilityIdentity::new("exec.run").unwrap(),
                scope: ScopeIdentity::new("workspace:default").unwrap(),
                operation: operation.into(),
            };
            assert_eq!(
                check_live_admission(&request),
                AdmissionVerdict::NotAdmitted {
                    reason: NotAdmittedReason::ProviderUnavailable
                },
                "operation {operation:?} must not be admitted without a live provider"
            );
        }
        assert_eq!(
            check_live_admission(&sample_request()),
            AdmissionVerdict::NotAdmitted {
                reason: NotAdmittedReason::ProviderUnavailable
            }
        );
    }

    #[test]
    fn verdict_carries_no_grant_or_token() {
        // Structural guard: a NotAdmitted verdict is a reason, not a
        // capability. There is no token, grant, expiry, or permission field
        // anywhere on the verdict shape.
        let verdict = check_live_admission(&sample_request());
        let json = serde_json::to_value(&verdict).unwrap();
        assert_eq!(json["verdict"], "NotAdmitted");
        assert_eq!(json["reason"]["reason"], "ProviderUnavailable");
        assert!(json.get("token").is_none());
        assert!(json.get("grant").is_none());
        assert!(json.get("permission").is_none());
        assert!(json.get("expiry").is_none());
    }

    #[test]
    fn authority_identities_reject_empty_and_control_values() {
        assert!(CapabilityIdentity::new("").is_err());
        assert!(ScopeIdentity::new("   ").is_err());
        assert!(AuthorityEvidenceReference::new("tethers://exec/01\n").is_err());
        assert!(AuthorityEvidenceReference::new("tethers://exec/01K9F82A").is_ok());
    }
}
