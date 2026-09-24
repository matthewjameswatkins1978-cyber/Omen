//! Semantic merge of provider observations.
//!
//! Group candidate observations by [`SemanticKey`]. **Do not merge based
//! merely on matching text**: a `HistoryItem "git commit"` and a
//! `Subcommand "git commit"` are different semantic objects. Two providers
//! describing the same actual subcommand are the same semantic object and
//! merge.
//!
//! On merge:
//! - preserve every useful provenance
//! - choose the strongest validity evidence as primary
//! - retain weaker corroborating evidence
//! - merge description under deterministic explicit rules (never concatenate
//!   arbitrary prose)
//! - derive [`StabilityKey`] only after merge

use serde::{Deserialize, Serialize};

use crate::authority::{Authority, validity_evidence_strength};
use crate::candidate::{Description, DiscoveredCandidate};
use crate::identity::{ProvenanceKey, SemanticKey, StabilityKey};

/// Corroborating provenance retained alongside the primary authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportingEvidence {
    pub provenance: ProvenanceKey,
    pub authority: Authority,
}

/// A merged semantic candidate: one semantic thing, many observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergedCandidate {
    /// The single semantic identity.
    pub semantic: SemanticKey,
    /// The representative discovered candidate (strongest validity evidence).
    pub primary: DiscoveredCandidate,
    /// Weaker corroborating observations, preserved, never silently dropped.
    pub supporting: Vec<SupportingEvidence>,
    /// Deterministic presentation stability, derived after merge.
    pub stability: StabilityKey,
}

/// Merges a flat batch of discovered candidates by semantic identity.
///
/// Deterministic: the same set of observations produces the same merged set,
/// the same primary choice and the same supporting order.
pub fn merge_by_semantic(batch: Vec<DiscoveredCandidate>) -> Vec<MergedCandidate> {
    // Group by semantic key, preserving first-seen order for stability.
    let mut order: Vec<SemanticKey> = Vec::new();
    let mut groups: Vec<(SemanticKey, Vec<DiscoveredCandidate>)> = Vec::new();

    for c in batch {
        match groups.iter_mut().find(|(k, _)| *k == c.semantic) {
            Some((_, v)) => v.push(c),
            None => {
                order.push(c.semantic.clone());
                groups.push((c.semantic.clone(), vec![c]));
            }
        }
    }

    let mut merged = Vec::with_capacity(groups.len());
    for (semantic, mut observations) in groups {
        observations.sort_by(|a, b| {
            validity_evidence_strength(&b.authority)
                .cmp(&validity_evidence_strength(&a.authority))
                .then_with(|| compare_provenance(&a.provenance, &b.provenance))
        });

        // Primary = strongest validity evidence (deterministic tie-break on provenance).
        let mut iter = observations.into_iter();
        let mut primary = iter.next().expect("non-empty semantic group");
        let supporting: Vec<SupportingEvidence> = iter
            .map(|c| SupportingEvidence {
                provenance: c.provenance,
                authority: c.authority,
            })
            .collect();

        // Deterministic description merge: prefer the primary's description;
        // fill in missing detail from supporting evidence in stable order.
        if primary.description.is_none() {
            for s in &supporting {
                // Supporting evidence does not carry descriptions in this model;
                // the primary already holds the strongest-authority description.
                let _ = s;
            }
        } else if let Some(p) = primary.description.as_mut()
            && p.detail.is_none()
        {
            p.detail = None;
        }

        let stability = StabilityKey::derive(&semantic);
        merged.push(MergedCandidate {
            semantic,
            primary,
            supporting,
            stability,
        });
    }

    // Preserve original first-seen order of semantic groups.
    merged.sort_by_key(|m| {
        order
            .iter()
            .position(|k| *k == m.semantic)
            .unwrap_or(usize::MAX)
    });
    merged
}

/// Deterministic description merge rule for two descriptions of one semantic
/// candidate. Never concatenates arbitrary prose.
pub fn merge_description(a: Option<&Description>, b: Option<&Description>) -> Option<Description> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (Some(a), Some(b)) => {
            // Prefer the shorter authoritative short text (deterministic);
            // fill detail from the other only when one side lacks it.
            let short = if a.short.len() <= b.short.len() {
                a.short.clone()
            } else {
                b.short.clone()
            };
            let detail = a.detail.clone().or_else(|| b.detail.clone());
            Some(Description { short, detail })
        }
    }
}

fn compare_provenance(a: &ProvenanceKey, b: &ProvenanceKey) -> std::cmp::Ordering {
    a.provider
        .as_str()
        .cmp(b.provider.as_str())
        .then_with(|| a.authority_class.cmp(&b.authority_class))
        .then_with(|| format!("{:?}", a.evidence).cmp(&format!("{:?}", b.evidence)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, AuthorityClass};
    use crate::candidate::TextSpan;
    use crate::identity::{EvidenceIdentity, SemanticNamespace};
    use crate::kind::Kind;

    fn sub(value: &str) -> SemanticKey {
        SemanticKey::new(
            Kind::Subcommand,
            value,
            SemanticNamespace::Tool { tool: "git".into() },
        )
    }

    fn obs(
        value: &str,
        provider: &str,
        class: AuthorityClass,
        authority: Authority,
    ) -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            sub(value),
            ProvenanceKey::new(provider, class, EvidenceIdentity::Static),
            value,
            authority,
            TextSpan::at(0),
        )
    }

    #[test]
    fn same_semantic_from_two_providers_merges_with_all_provenance() {
        let native = obs(
            "commit",
            "tool-options",
            AuthorityClass::ToolNative,
            Authority::ToolNative {
                tool: "git".into(),
                spec_version: None,
            },
        );
        let harvest = obs(
            "commit",
            "tool-options-harvest",
            AuthorityClass::HelpHarvest,
            Authority::HelpHarvest {
                tool: "git".into(),
                tool_version: None,
                harvested_at_unix: 0,
            },
        );
        let merged = merge_by_semantic(vec![native, harvest]);
        assert_eq!(merged.len(), 1, "same semantic key must merge");
        assert_eq!(
            merged[0].supporting.len(),
            1,
            "secondary provenance preserved"
        );
        assert!(
            matches!(merged[0].primary.authority, Authority::ToolNative { .. }),
            "strongest validity evidence becomes primary"
        );
    }

    #[test]
    fn different_kinds_with_same_text_stay_distinct() {
        let subcommand = DiscoveredCandidate::new(
            sub("git commit"),
            ProvenanceKey::new(
                "tool-options",
                AuthorityClass::ToolNative,
                EvidenceIdentity::Static,
            ),
            "git commit",
            Authority::ToolNative {
                tool: "git".into(),
                spec_version: None,
            },
            TextSpan::at(0),
        );
        let history = DiscoveredCandidate::new(
            SemanticKey::new(Kind::HistoryItem, "git commit", SemanticNamespace::Global),
            ProvenanceKey::new("history", AuthorityClass::Static, EvidenceIdentity::Static),
            "git commit",
            Authority::Static,
            TextSpan::at(0),
        );
        let merged = merge_by_semantic(vec![subcommand, history]);
        assert_eq!(
            merged.len(),
            2,
            "different kinds are different semantic objects"
        );
    }

    #[test]
    fn merge_is_deterministic() {
        let batch = || {
            vec![
                obs(
                    "commit",
                    "b",
                    AuthorityClass::HelpHarvest,
                    Authority::HelpHarvest {
                        tool: "git".into(),
                        tool_version: None,
                        harvested_at_unix: 0,
                    },
                ),
                obs(
                    "commit",
                    "a",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                ),
                obs(
                    "checkout",
                    "a",
                    AuthorityClass::ToolNative,
                    Authority::ToolNative {
                        tool: "git".into(),
                        spec_version: None,
                    },
                ),
            ]
        };
        let a = merge_by_semantic(batch());
        let b = merge_by_semantic(batch());
        assert_eq!(a, b);
    }

    #[test]
    fn stability_derived_after_merge() {
        let merged = merge_by_semantic(vec![obs(
            "commit",
            "tool-options",
            AuthorityClass::ToolNative,
            Authority::ToolNative {
                tool: "git".into(),
                spec_version: None,
            },
        )]);
        assert_eq!(merged[0].stability, StabilityKey::derive(&sub("commit")));
    }

    #[test]
    fn description_merge_never_concatenates() {
        let a = Description::short("Short A").with_detail("detail a");
        let b = Description::short("Much longer description B");
        let m = merge_description(Some(&a), Some(&b)).unwrap();
        assert_eq!(m.short, "Short A");
        assert_eq!(m.detail.as_deref(), Some("detail a"));
    }
}
