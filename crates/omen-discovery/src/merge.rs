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
///
/// Supporting evidence travels all the way to the ranked state so the human
/// projection, the machine projection and telemetry can inspect the complete
/// evidence set (merge preserves all useful provenance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportingEvidence {
    pub provenance: ProvenanceKey,
    pub authority: Authority,
    /// The description this observation carried, if any. Preserved so the
    /// deterministic description-merge rules can run *before* evidence is
    /// dropped; it is never concatenated with any other description.
    pub description: Option<Description>,
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
                description: c.description,
            })
            .collect();

        // Deterministic description resolution, run BEFORE any evidence is
        // dropped. The primary (strongest validity evidence) wins wherever it
        // has a description; supporting evidence may only *fill missing*
        // short/detail slots in stable strength order. Never concatenate,
        // never pick shortest-regardless-of-evidence.
        resolve_description(&mut primary, &supporting);

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

/// Deterministic description merge for two descriptions of one semantic
/// candidate.
///
/// `a` is the primary (strongest validity evidence) description: it wins
/// wholesale. `b` may only fill a *missing* `detail` slot. Never concatenates
/// arbitrary prose; never selects text independently of evidence.
pub fn merge_description(a: Option<&Description>, b: Option<&Description>) -> Option<Description> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (Some(a), Some(b)) => {
            let mut merged = a.clone();
            if merged.detail.is_none() {
                merged.detail = b.detail.clone();
            }
            Some(merged)
        }
    }
}

/// Resolves the primary candidate's description from the preserved supporting
/// evidence, under the accepted deterministic rules:
///
/// - the primary description (strongest validity evidence) is kept whenever
///   present;
/// - a missing primary description is filled from the first supporting
///   observation that has one (stable strength order);
/// - a missing primary `detail` is filled likewise;
/// - descriptions are never concatenated and prose is never selected by
///   length.
fn resolve_description(primary: &mut DiscoveredCandidate, supporting: &[SupportingEvidence]) {
    // `supporting` is ordered strongest-first, so a plain `find_map` is the
    // deterministic choice: first supporting observation with a description.
    if primary.description.is_none() {
        primary.description = supporting.iter().find_map(|s| s.description.clone());
        return;
    }
    if primary
        .description
        .as_ref()
        .is_some_and(|d| d.detail.is_none())
        && let Some(detail) = supporting
            .iter()
            .find_map(|s| s.description.as_ref().and_then(|d| d.detail.clone()))
        && let Some(d) = primary.description.as_mut()
    {
        d.detail = Some(detail);
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

    #[test]
    fn supporting_description_is_preserved_on_merge() {
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
        )
        .with_description(Description::short("harvested description"));
        let merged = merge_by_semantic(vec![native, harvest]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].supporting.len(), 1);
        assert_eq!(
            merged[0].supporting[0].description.as_ref().unwrap().short,
            "harvested description",
            "supporting descriptions must survive the merge for later rules"
        );
    }

    #[test]
    fn primary_without_description_adopts_supporting_description() {
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
        )
        .with_description(Description::short("harvested description"));
        let merged = merge_by_semantic(vec![native, harvest]);
        assert_eq!(
            merged[0].primary.description.as_ref().unwrap().short,
            "harvested description",
            "missing primary description fills from the strongest supporting evidence"
        );
    }

    #[test]
    fn primary_description_wins_and_detail_fills_from_supporting() {
        let native = obs(
            "commit",
            "tool-options",
            AuthorityClass::ToolNative,
            Authority::ToolNative {
                tool: "git".into(),
                spec_version: None,
            },
        )
        .with_description(Description::short("primary short"));
        let harvest = obs(
            "commit",
            "tool-options-harvest",
            AuthorityClass::HelpHarvest,
            Authority::HelpHarvest {
                tool: "git".into(),
                tool_version: None,
                harvested_at_unix: 0,
            },
        )
        .with_description(Description::short("secondary short").with_detail("secondary detail"));
        let merged = merge_by_semantic(vec![native, harvest]);
        let d = merged[0].primary.description.as_ref().unwrap();
        assert_eq!(d.short, "primary short", "primary description wins");
        assert_eq!(
            d.detail.as_deref(),
            Some("secondary detail"),
            "supporting fills only the missing detail slot"
        );
    }

    #[test]
    fn description_merge_is_deterministic_across_runs() {
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
                )
                .with_description(Description::short("B").with_detail("b-detail")),
                obs(
                    "commit",
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
        assert_eq!(
            a[0].primary.description.as_ref().unwrap().short,
            "B",
            "description filled from strongest supporting evidence"
        );
    }
}
