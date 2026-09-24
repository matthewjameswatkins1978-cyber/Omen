//! Identity model for discovery.
//!
//! Three distinct identities answer three distinct questions. They are never
//! interchangeable and each has exactly one job:
//!
//! - [`SemanticKey`] — **WHAT** real discoverable thing is this? Drives
//!   candidate merge/dedup. Contains no provider, authority or rank.
//! - [`ProvenanceKey`] — **WHERE** did this observation come from? Preserved
//!   in full when multiple providers support one semantic candidate.
//! - [`StabilityKey`] — **HOW** do we keep presentation stable? Derived after
//!   semantic merge. Never provider supplied. Never validity evidence.

use serde::{Deserialize, Serialize};

use crate::authority::AuthorityClass;
use crate::kind::Kind;
use crate::provider::ProviderId;

/// Normalised insert text used for semantic equality.
///
/// Normalisation is deliberately conservative: Unicode case-fold on ASCII plus
/// separator canonicalisation is *not* applied here because path case is
/// meaningful on some platforms. Semantic equality is therefore
/// case-sensitive for values, and callers that need case-insensitive matching
/// use the matcher ([`crate::matcher::MatchQuality::Normalised`]) — matching is
/// not identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CanonicalValue(pub String);

impl CanonicalValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The semantic scope a candidate belongs to.
///
/// Two candidates with the same [`Kind`] and [`CanonicalValue`] but different
/// namespaces are **different semantic objects** and must not merge. A
/// `HistoryItem "git commit"` and a `Subcommand "git commit"` differ by both
/// kind and namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SemanticNamespace {
    /// No enclosing scope (shell intrinsics, PATH command names).
    Global,
    /// Inside a specific external tool's grammar.
    Tool { tool: String },
    /// Inside a filesystem subtree.
    Path { root: String },
    /// Inside a specific Omen semantic action's argument grammar.
    Action { action: String },
    /// Inside a project/workspace scope.
    Project { project: String },
}

/// **WHAT** discoverable thing is this?
///
/// Semantic equality drives candidate merge/dedup. This key deliberately
/// excludes provider identity, authority identity and presentation rank.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SemanticKey {
    pub kind: Kind,
    pub canonical_value: CanonicalValue,
    pub namespace: SemanticNamespace,
}

impl SemanticKey {
    pub fn new(kind: Kind, value: impl Into<String>, namespace: SemanticNamespace) -> Self {
        Self {
            kind,
            canonical_value: CanonicalValue::new(value),
            namespace,
        }
    }
}

/// Identity of the evidence that produced an observation.
///
/// Two observations of the same semantic candidate from different evidence
/// sources carry different [`EvidenceIdentity`]s and are both preserved.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EvidenceIdentity {
    /// Structured tool-native spec (clap/`__complete`/Omen registry).
    ToolNative {
        tool: String,
        spec_version: Option<String>,
    },
    /// Declarative inert spec digest.
    InstalledSpec { digest: String },
    /// Deterministic bounded help harvest.
    HelpHarvest {
        tool_version: Option<String>,
        harvested_at_unix: u64,
    },
    /// Bounded filesystem observation.
    Filesystem {
        root: String,
        mtime_unix: Option<u64>,
    },
    /// Omen internal fact.
    OmenFact { fact_id: String },
    /// Environment-derived fact.
    Environment { fact: String },
    /// Compile-time / static knowledge.
    Static,
}

/// **WHERE** did this observation come from?
///
/// Preserved in full on merge. Never used as presentation ranking.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProvenanceKey {
    pub provider: ProviderId,
    pub authority_class: AuthorityClass,
    pub evidence: EvidenceIdentity,
}

impl ProvenanceKey {
    pub fn new(
        provider: impl Into<String>,
        authority_class: AuthorityClass,
        evidence: EvidenceIdentity,
    ) -> Self {
        Self {
            provider: ProviderId::new(provider),
            authority_class,
            evidence,
        }
    }
}

/// **HOW** do we keep presentation stable across refreshes?
///
/// Derived deterministically *after* semantic merge from the merged
/// [`SemanticKey`]. Never provider supplied. Never validity evidence. Used
/// only as the final ordering tie-breaker so the menu does not flap between
/// keystrokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StabilityKey(pub u64);

impl StabilityKey {
    /// Deterministic derivation from a merged semantic key.
    ///
    /// FNV-1a over the stable textual projection of the semantic key. No
    /// randomness, no process identity: the same semantic candidate always
    /// derives the same stability key.
    pub fn derive(key: &SemanticKey) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |bytes: &[u8]| {
            for b in bytes {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        mix(format!("{:?}", key.kind).as_bytes());
        mix(&[0x1f]);
        mix(key.canonical_value.as_str().as_bytes());
        mix(&[0x1f]);
        mix(format!("{:?}", key.namespace).as_bytes());
        Self(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(v: &str) -> SemanticKey {
        SemanticKey::new(
            Kind::Subcommand,
            v,
            SemanticNamespace::Tool { tool: "git".into() },
        )
    }

    #[test]
    fn semantic_key_ignores_provider_and_authority() {
        let a = sub("commit");
        let b = sub("commit");
        assert_eq!(a, b);
    }

    #[test]
    fn semantic_key_separates_kind_even_with_same_text() {
        let subcommand = sub("git commit");
        let history = SemanticKey::new(Kind::HistoryItem, "git commit", SemanticNamespace::Global);
        assert_ne!(
            subcommand, history,
            "same text, different kind => different thing"
        );
    }

    #[test]
    fn semantic_key_separates_namespace() {
        let git = sub("build");
        let cargo = SemanticKey::new(
            Kind::Subcommand,
            "build",
            SemanticNamespace::Tool {
                tool: "cargo".into(),
            },
        );
        assert_ne!(git, cargo);
    }

    #[test]
    fn stability_key_is_deterministic() {
        let k = sub("commit");
        assert_eq!(StabilityKey::derive(&k), StabilityKey::derive(&k));
    }

    #[test]
    fn stability_key_differs_for_different_semantics() {
        let a = StabilityKey::derive(&sub("commit"));
        let b = StabilityKey::derive(&sub("checkout"));
        assert_ne!(a, b);
    }

    #[test]
    fn provenance_key_distinguishes_evidence() {
        let native = ProvenanceKey::new(
            "tool-options",
            AuthorityClass::ToolNative,
            EvidenceIdentity::ToolNative {
                tool: "git".into(),
                spec_version: Some("2".into()),
            },
        );
        let harvest = ProvenanceKey::new(
            "tool-options",
            AuthorityClass::HelpHarvest,
            EvidenceIdentity::HelpHarvest {
                tool_version: Some("2".into()),
                harvested_at_unix: 0,
            },
        );
        assert_ne!(native, harvest);
    }
}
