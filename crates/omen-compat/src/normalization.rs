use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Explicit byte transformations applied only to comparison projections.
/// Raw observations remain unchanged and are never replaced by normalized data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NormalizationRule {
    /// Convert CRLF pairs to LF. Bare CR bytes are preserved.
    CrLfToLf,
    /// Replace one explicit byte sequence. Empty patterns are rejected.
    Replace { from: Vec<u8>, to: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NormalizationProfile {
    rules: Vec<NormalizationRule>,
}

impl NormalizationProfile {
    pub fn new(rules: Vec<NormalizationRule>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[NormalizationRule] {
        &self.rules
    }

    /// Apply only the listed rules, in order, with explicit output and work bounds.
    pub fn apply(
        &self,
        input: &[u8],
        max_output_bytes: usize,
        max_replacements: usize,
    ) -> Result<NormalizedBytes, NormalizationError> {
        if input.len() > max_output_bytes {
            return Err(NormalizationError::OutputLimitExceeded {
                limit: max_output_bytes,
            });
        }

        let mut bytes = input.to_vec();
        let mut applications = Vec::with_capacity(self.rules.len());
        let mut replacements_left = max_replacements;

        for (rule_index, rule) in self.rules.iter().enumerate() {
            let (from, to) = match rule {
                NormalizationRule::CrLfToLf => (b"\r\n".as_slice(), b"\n".as_slice()),
                NormalizationRule::Replace { from, to } => (from.as_slice(), to.as_slice()),
            };
            if from.is_empty() {
                return Err(NormalizationError::EmptyPattern { rule_index });
            }

            let (next, count) = replace_all(&bytes, from, to, replacements_left, max_output_bytes)?;
            if next.len() > max_output_bytes {
                return Err(NormalizationError::OutputLimitExceeded {
                    limit: max_output_bytes,
                });
            }
            replacements_left -= count;
            bytes = next;
            applications.push(RuleApplication {
                rule_index,
                replacements: count,
            });
        }

        Ok(NormalizedBytes {
            bytes,
            applications,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedBytes {
    pub bytes: Vec<u8>,
    pub applications: Vec<RuleApplication>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleApplication {
    pub rule_index: usize,
    pub replacements: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NormalizationError {
    #[error("normalization rule {rule_index} has an empty search pattern")]
    EmptyPattern { rule_index: usize },
    #[error("normalization output exceeded the explicit {limit}-byte bound")]
    OutputLimitExceeded { limit: usize },
    #[error("normalization exceeded the explicit replacement-count bound")]
    ReplacementLimitExceeded,
}

fn replace_all(
    input: &[u8],
    from: &[u8],
    to: &[u8],
    max_replacements: usize,
    max_output_bytes: usize,
) -> Result<(Vec<u8>, usize), NormalizationError> {
    let mut output = Vec::with_capacity(input.len());
    let mut remaining = input;
    let mut count = 0usize;

    while let Some(index) = remaining
        .windows(from.len())
        .position(|window| window == from)
    {
        if count == max_replacements {
            return Err(NormalizationError::ReplacementLimitExceeded);
        }
        if output.len().saturating_add(index).saturating_add(to.len()) > max_output_bytes {
            return Err(NormalizationError::OutputLimitExceeded {
                limit: max_output_bytes,
            });
        }
        output.extend_from_slice(&remaining[..index]);
        output.extend_from_slice(to);
        remaining = &remaining[index + from.len()..];
        count += 1;
    }

    if output.len().saturating_add(remaining.len()) > max_output_bytes {
        return Err(NormalizationError::OutputLimitExceeded {
            limit: max_output_bytes,
        });
    }
    output.extend_from_slice(remaining);
    Ok((output, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_explicit_ordered_and_byte_preserving() {
        let profile = NormalizationProfile::new(vec![
            NormalizationRule::CrLfToLf,
            NormalizationRule::Replace {
                from: b"C:\\build\\temp".to_vec(),
                to: b"<ROOT>".to_vec(),
            },
        ]);

        let normalized = profile
            .apply(b"one\r\ntwo C:\\build\\temp\rthree", 64, 8)
            .unwrap();

        assert_eq!(normalized.bytes, b"one\ntwo <ROOT>\rthree");
        assert_eq!(normalized.applications[0].replacements, 1);
        assert_eq!(normalized.applications[1].replacements, 1);
    }

    #[test]
    fn normalization_rejects_empty_patterns_and_unbounded_growth() {
        let empty = NormalizationProfile::new(vec![NormalizationRule::Replace {
            from: Vec::new(),
            to: b"x".to_vec(),
        }]);
        assert_eq!(
            empty.apply(b"data", 16, 4),
            Err(NormalizationError::EmptyPattern { rule_index: 0 })
        );

        let growth = NormalizationProfile::new(vec![NormalizationRule::Replace {
            from: b"a".to_vec(),
            to: b"longer".to_vec(),
        }]);
        assert_eq!(
            growth.apply(b"aaa", 8, 8),
            Err(NormalizationError::OutputLimitExceeded { limit: 8 })
        );
    }

    #[test]
    fn normalization_replacement_count_is_bounded() {
        let profile = NormalizationProfile::new(vec![NormalizationRule::Replace {
            from: b"a".to_vec(),
            to: b"b".to_vec(),
        }]);
        assert_eq!(
            profile.apply(b"aaa", 8, 2),
            Err(NormalizationError::ReplacementLimitExceeded)
        );
    }
}
