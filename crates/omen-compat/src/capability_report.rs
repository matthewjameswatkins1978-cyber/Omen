use crate::core::{Capability, EvidenceGrade, Platform};
use serde::{Deserialize, Serialize};

use thiserror::Error;

pub const CAPABILITY_REPORT_SCHEMA: &str = "omen.compat.capability-report.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Available,
    Unsupported,
    Unavailable,
    NotTested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityClaim {
    pub capability: Capability,
    pub status: CapabilityStatus,
    pub evidence_grade: EvidenceGrade,
    /// Stable evidence identifier, not a secret-bearing path or raw trace.
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub schema: String,
    pub platform: Platform,
    pub claims: Vec<CapabilityClaim>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CapabilityReportError {
    #[error("capability {0:?} occurs more than once")]
    Duplicate(Capability),
    #[error("capability {capability:?} does not belong in a {platform:?} report")]
    PlatformMismatch {
        capability: Capability,
        platform: Platform,
    },
    #[error("available capability {0:?} requires strong evidence and an evidence id")]
    UnsupportedAvailableClaim(Capability),
    #[error("not-tested capability {0:?} cannot carry evidence")]
    EvidenceForUntested(Capability),
    #[error("evidence id must be 1..=128 printable non-whitespace bytes")]
    InvalidEvidenceId,
}

impl CapabilityReport {
    pub fn new(
        platform: Platform,
        claims: Vec<CapabilityClaim>,
    ) -> Result<Self, CapabilityReportError> {
        let mut seen: Vec<Capability> = Vec::new();
        for claim in &claims {
            if seen.contains(&claim.capability) {
                return Err(CapabilityReportError::Duplicate(claim.capability));
            }
            seen.push(claim.capability);
            if !belongs_to_report(claim.capability, platform) {
                return Err(CapabilityReportError::PlatformMismatch {
                    capability: claim.capability,
                    platform,
                });
            }
            if claim.evidence_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 128
                    || id.trim() != id
                    || id.chars().any(char::is_control)
            }) {
                return Err(CapabilityReportError::InvalidEvidenceId);
            }
            match claim.status {
                CapabilityStatus::Available
                    if claim.evidence_grade != EvidenceGrade::Strong
                        || claim.evidence_id.is_none() =>
                {
                    return Err(CapabilityReportError::UnsupportedAvailableClaim(
                        claim.capability,
                    ));
                }
                CapabilityStatus::NotTested
                    if claim.evidence_grade != EvidenceGrade::Unavailable
                        || claim.evidence_id.is_some() =>
                {
                    return Err(CapabilityReportError::EvidenceForUntested(claim.capability));
                }
                _ => {}
            }
        }

        Ok(Self {
            schema: CAPABILITY_REPORT_SCHEMA.to_owned(),
            platform,
            claims,
        })
    }
}

fn belongs_to_report(capability: Capability, platform: Platform) -> bool {
    match capability {
        Capability::PosixProcessGroup
        | Capability::PosixControllingTerminal
        | Capability::PosixForegroundProcessGroup
        | Capability::PosixSignals
        | Capability::PosixTermios => platform == Platform::Posix,
        Capability::Win32Console
        | Capability::Win32ConsoleControl
        | Capability::Win32JobObject
        | Capability::Win32ConPty => platform == Platform::Windows,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_claim_requires_strong_evidence_identity() {
        let valid = CapabilityClaim {
            capability: Capability::ProcessSpawn,
            status: CapabilityStatus::Available,
            evidence_grade: EvidenceGrade::Strong,
            evidence_id: Some("tier1.process-spawn".into()),
        };
        let report = CapabilityReport::new(Platform::Windows, vec![valid.clone()]).unwrap();
        assert_eq!(report.schema, CAPABILITY_REPORT_SCHEMA);
        assert!(matches!(
            CapabilityReport::new(
                Platform::Windows,
                vec![CapabilityClaim {
                    evidence_id: None,
                    ..valid
                }]
            ),
            Err(CapabilityReportError::UnsupportedAvailableClaim(
                Capability::ProcessSpawn
            ))
        ));
    }

    #[test]
    fn platform_mismatch_and_duplicate_capabilities_are_rejected() {
        let posix = CapabilityClaim {
            capability: Capability::PosixSignals,
            status: CapabilityStatus::NotTested,
            evidence_grade: EvidenceGrade::Unavailable,
            evidence_id: None,
        };
        assert!(matches!(
            CapabilityReport::new(Platform::Windows, vec![posix.clone()]),
            Err(CapabilityReportError::PlatformMismatch { .. })
        ));

        let portable = CapabilityClaim {
            capability: Capability::Stdout,
            ..posix
        };
        assert_eq!(
            CapabilityReport::new(Platform::Windows, vec![portable.clone(), portable]),
            Err(CapabilityReportError::Duplicate(Capability::Stdout))
        );
    }

    #[test]
    fn untested_is_distinct_from_unavailable_and_cannot_claim_evidence() {
        let claim = CapabilityClaim {
            capability: Capability::Stdin,
            status: CapabilityStatus::NotTested,
            evidence_grade: EvidenceGrade::Unavailable,
            evidence_id: Some("invented".into()),
        };
        assert_eq!(
            CapabilityReport::new(Platform::Posix, vec![claim]),
            Err(CapabilityReportError::EvidenceForUntested(
                Capability::Stdin
            ))
        );
    }
}
