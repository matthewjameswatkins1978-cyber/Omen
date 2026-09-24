//! Deterministic staged matcher.
//!
//! M1 active stages are **cheap and deterministic**: [`MatchQuality::Exact`],
//! [`MatchQuality::Prefix`], [`MatchQuality::TokenPrefix`],
//! [`MatchQuality::Normalised`].
//!
//! [`MatchQuality::Fuzzy`] and [`MatchQuality::Semantic`] exist in the
//! vocabulary as seams for M2 but are **not activated** in M1. The enum
//! existing is not a reason to turn fuzzy matching on.
//!
//! Matching is not identity: it produces [`MatchedCandidate`](crate::candidate::MatchedCandidate)
//! and highlight indices. It never changes candidate validity.

use serde::{Deserialize, Serialize};

use crate::candidate::{DiscoveredCandidate, MatchedCandidate};

/// How closely a candidate matches the active query.
///
/// Ordered from strongest to weakest. The ordinal is used by the ranker as the
/// dominant presentation signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MatchQuality {
    /// Candidate equals the query.
    Exact,
    /// Candidate starts with the query.
    Prefix,
    /// Every query token is a prefix of some candidate token.
    TokenPrefix,
    /// Case/separator-insensitive prefix.
    Normalised,
    /// Subsequence match (M2 seam, not activated in M1).
    Fuzzy,
    /// Explanatory/embedding match (M2 seam, not activated in M1).
    Semantic,
}

impl MatchQuality {
    /// Stronger matches sort first. Lower ordinal = stronger.
    pub fn rank(self) -> u8 {
        match self {
            MatchQuality::Exact => 0,
            MatchQuality::Prefix => 1,
            MatchQuality::TokenPrefix => 2,
            MatchQuality::Normalised => 3,
            MatchQuality::Fuzzy => 4,
            MatchQuality::Semantic => 5,
        }
    }

    /// Whether M1 may produce this quality. Fuzzy/Semantic are reserved.
    pub fn is_active_in_m1(self) -> bool {
        matches!(
            self,
            MatchQuality::Exact
                | MatchQuality::Prefix
                | MatchQuality::TokenPrefix
                | MatchQuality::Normalised
        )
    }
}

/// Central matcher. Pure, deterministic, no I/O.
pub struct Matcher;

impl Matcher {
    /// Matches one discovered candidate against the active query.
    ///
    /// Returns `None` when the candidate does not match at all, so the caller
    /// may drop it. `match_indices` are byte-independent **char** indices into
    /// the candidate's insert text, for highlight only.
    pub fn match_one(candidate: &DiscoveredCandidate, query: &str) -> Option<MatchedCandidate> {
        let text = candidate.value.insert.as_str();
        let (quality, indices) = classify(text, query)?;
        Some(MatchedCandidate::new(candidate.clone(), quality, indices))
    }

    /// Matches a whole batch, dropping non-matches.
    pub fn match_all(candidates: Vec<DiscoveredCandidate>, query: &str) -> Vec<MatchedCandidate> {
        candidates
            .into_iter()
            .filter_map(|c| Self::match_one(&c, query))
            .collect()
    }
}

fn classify(text: &str, query: &str) -> Option<(MatchQuality, Vec<usize>)> {
    if query.is_empty() {
        // Empty query matches everything at the weakest deterministic stage.
        return Some((MatchQuality::Prefix, Vec::new()));
    }
    if text == query {
        return Some((MatchQuality::Exact, (0..text.chars().count()).collect()));
    }
    if text.starts_with(query) {
        return Some((MatchQuality::Prefix, (0..query.chars().count()).collect()));
    }

    // Token-prefix: every whitespace-separated query token is a prefix of some
    // whitespace-separated candidate token.
    let text_tokens: Vec<(usize, &str)> = tokens_with_start(text);
    let query_tokens: Vec<&str> = query.split_whitespace().collect();
    if !query_tokens.is_empty() {
        let mut all = true;
        let mut indices = Vec::new();
        for qt in &query_tokens {
            let mut found = false;
            for (start, tt) in &text_tokens {
                if tt.starts_with(qt) {
                    for i in 0..qt.chars().count() {
                        indices.push(start + i);
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                all = false;
                break;
            }
        }
        if all {
            indices.sort_unstable();
            indices.dedup();
            return Some((MatchQuality::TokenPrefix, indices));
        }
    }

    // Normalised (case-insensitive) prefix.
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    if text_lower.starts_with(&query_lower) {
        return Some((
            MatchQuality::Normalised,
            (0..query.chars().count()).collect(),
        ));
    }

    // Fuzzy and Semantic are reserved for M2 and are NOT produced here.
    None
}

fn tokens_with_start(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut current_start = 0usize;
    let mut in_token = false;
    for (i, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if in_token {
                out.push((current_start, &text[current_start..i]));
                in_token = false;
            }
        } else if !in_token {
            current_start = i;
            in_token = true;
        }
    }
    if in_token {
        out.push((current_start, &text[current_start..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, AuthorityClass};
    use crate::identity::{EvidenceIdentity, ProvenanceKey, SemanticKey, SemanticNamespace};
    use crate::kind::Kind;

    fn cand(value: &str) -> DiscoveredCandidate {
        DiscoveredCandidate::new(
            SemanticKey::new(
                Kind::Option,
                value,
                SemanticNamespace::Tool { tool: "git".into() },
            ),
            ProvenanceKey::new(
                "tool-options",
                AuthorityClass::ToolNative,
                EvidenceIdentity::Static,
            ),
            value,
            Authority::ToolNative {
                tool: "git".into(),
                spec_version: None,
            },
            crate::candidate::TextSpan::at(0),
        )
    }

    #[test]
    fn exact_beats_prefix_beats_token_beats_normalised() {
        assert!(MatchQuality::Exact.rank() < MatchQuality::Prefix.rank());
        assert!(MatchQuality::Prefix.rank() < MatchQuality::TokenPrefix.rank());
        assert!(MatchQuality::TokenPrefix.rank() < MatchQuality::Normalised.rank());
    }

    #[test]
    fn exact_match() {
        let m = Matcher::match_one(&cand("--verbose"), "--verbose").unwrap();
        assert_eq!(m.match_quality, MatchQuality::Exact);
    }

    #[test]
    fn prefix_match() {
        let m = Matcher::match_one(&cand("--verbose"), "--verb").unwrap();
        assert_eq!(m.match_quality, MatchQuality::Prefix);
        assert_eq!(m.match_indices.len(), "--verb".chars().count());
    }

    #[test]
    fn token_prefix_match() {
        let multi = DiscoveredCandidate::new(
            SemanticKey::new(Kind::ArgumentValue, "git commit", SemanticNamespace::Global),
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
            crate::candidate::TextSpan::at(0),
        );
        let m = Matcher::match_one(&multi, "com git").unwrap();
        assert_eq!(m.match_quality, MatchQuality::TokenPrefix);
    }

    #[test]
    fn normalised_match() {
        let m = Matcher::match_one(&cand("--Verbose"), "--verb").unwrap();
        assert_eq!(m.match_quality, MatchQuality::Normalised);
    }

    #[test]
    fn no_match_returns_none() {
        assert!(Matcher::match_one(&cand("--verbose"), "zzz").is_none());
    }

    #[test]
    fn m1_never_produces_fuzzy_or_semantic() {
        // A subsequence like "vbe" must NOT match in M1.
        assert!(Matcher::match_one(&cand("--verbose"), "vbe").is_none());
    }

    #[test]
    fn fuzzy_and_semantic_are_inactive_in_m1() {
        assert!(!MatchQuality::Fuzzy.is_active_in_m1());
        assert!(!MatchQuality::Semantic.is_active_in_m1());
        assert!(MatchQuality::Exact.is_active_in_m1());
    }

    #[test]
    fn empty_query_matches_everything() {
        let m = Matcher::match_one(&cand("--verbose"), "").unwrap();
        assert_eq!(m.match_quality, MatchQuality::Prefix);
    }

    #[test]
    fn match_all_drops_non_matches() {
        let batch = vec![cand("--verbose"), cand("--quiet")];
        let out = Matcher::match_all(batch, "--ver");
        assert_eq!(out.len(), 1);
    }
}
