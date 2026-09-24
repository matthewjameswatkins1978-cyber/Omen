//! Typed, non-optional validity authority.
//!
//! **Core law:** *Intelligence may rank, explain and reveal. Authority
//! determines what is real.*
//!
//! Every [`crate::candidate::DiscoveredCandidate`] carries concrete authority
//! evidence. There is deliberately **no** `Guess`, `Ai` or `ProbablyReal`
//! variant: an AI may later explain a candidate, but it can never assert that
//! a candidate exists.
//!
//! Authority strength is **not** a global presentation score. It resolves
//! competing validity evidence for the *same* [`crate::identity::SemanticKey`]
//! only. Presentation ordering of distinct valid candidates uses match quality,
//! context and declared order — see [`crate::rank`].

use serde::{Deserialize, Serialize};

/// Coarse authority class.
///
/// Declared by a provider as a *capability* ("I may produce these classes");
/// the concrete authority lives on each candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AuthorityClass {
    /// Structured, tool-native authoritative source.
    ToolNative,
    /// Declarative inert installed spec.
    InstalledSpec,
    /// Deterministic bounded help harvest.
    HelpHarvest,
    /// Local filesystem truth.
    Filesystem,
    /// Omen's own typed facts.
    OmenFact,
    /// Environment-derived fact.
    Environment,
    /// Compile-time / static knowledge.
    Static,
}

/// Concrete validity evidence carried by a candidate.
///
/// This is *per candidate*, not per provider: a single tool-option provider may
/// emit `ToolNative` for one option (answered by a structured spec) and
/// `HelpHarvest` for another (answered by the harvest fallback) in the same
/// batch.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Authority {
    /// Structured tool-native metadata (clap, `__complete` protocol, Omen registry).
    ToolNative {
        tool: String,
        spec_version: Option<String>,
    },
    /// Declarative inert installed spec, parsed under a bounded explicit schema.
    InstalledSpec { origin: String, digest: String },
    /// Strict bounded help harvest. Lower confidence; never permission to invent syntax.
    HelpHarvest {
        tool: String,
        tool_version: Option<String>,
        harvested_at_unix: u64,
    },
    /// Bounded filesystem observation.
    Filesystem { root: String, observed_at_unix: u64 },
    /// Omen internal fact.
    OmenFact { fact_id: String },
    /// Environment-derived fact.
    Environment { fact: String },
    /// Compile-time / static knowledge.
    Static,
}

impl Authority {
    pub fn class(&self) -> AuthorityClass {
        match self {
            Authority::ToolNative { .. } => AuthorityClass::ToolNative,
            Authority::InstalledSpec { .. } => AuthorityClass::InstalledSpec,
            Authority::HelpHarvest { .. } => AuthorityClass::HelpHarvest,
            Authority::Filesystem { .. } => AuthorityClass::Filesystem,
            Authority::OmenFact { .. } => AuthorityClass::OmenFact,
            Authority::Environment { .. } => AuthorityClass::Environment,
            Authority::Static => AuthorityClass::Static,
        }
    }
}

/// Strength of validity evidence, used **only** to choose the primary evidence
/// when several providers observe the *same* semantic candidate.
///
/// This is never a presentation/importance score. A filesystem directory must
/// not be demoted below an unrelated tool-native option simply because their
/// authority classes differ; they are different semantic objects and are
/// ordered by match quality and context, not by this function.
pub fn validity_evidence_strength(authority: &Authority) -> u8 {
    match authority {
        Authority::ToolNative { .. } => 6,
        Authority::InstalledSpec { .. } => 5,
        Authority::HelpHarvest { .. } => 4,
        Authority::OmenFact { .. } => 3,
        Authority::Filesystem { .. } => 2,
        Authority::Environment { .. } => 1,
        Authority::Static => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strength_orders_competing_evidence_for_same_semantic_candidate() {
        let native = Authority::ToolNative {
            tool: "git".into(),
            spec_version: None,
        };
        let harvest = Authority::HelpHarvest {
            tool: "git".into(),
            tool_version: None,
            harvested_at_unix: 0,
        };
        assert!(validity_evidence_strength(&native) > validity_evidence_strength(&harvest));
    }

    #[test]
    fn every_authority_has_a_class() {
        let all = [
            Authority::ToolNative {
                tool: "t".into(),
                spec_version: None,
            },
            Authority::InstalledSpec {
                origin: "o".into(),
                digest: "d".into(),
            },
            Authority::HelpHarvest {
                tool: "t".into(),
                tool_version: None,
                harvested_at_unix: 0,
            },
            Authority::Filesystem {
                root: "r".into(),
                observed_at_unix: 0,
            },
            Authority::OmenFact {
                fact_id: "f".into(),
            },
            Authority::Environment { fact: "e".into() },
            Authority::Static,
        ];
        for a in all {
            let _ = a.class();
        }
    }

    #[test]
    fn no_guess_or_ai_authority_exists() {
        // Structural proof: the enum has exactly these variants. Adding a
        // Guess/Ai variant would require editing this test.
        let native = Authority::ToolNative {
            tool: "t".into(),
            spec_version: None,
        };
        match native {
            Authority::ToolNative { .. }
            | Authority::InstalledSpec { .. }
            | Authority::HelpHarvest { .. }
            | Authority::Filesystem { .. }
            | Authority::OmenFact { .. }
            | Authority::Environment { .. }
            | Authority::Static => {}
        }
    }
}
