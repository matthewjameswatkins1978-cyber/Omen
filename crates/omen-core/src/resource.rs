use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Supported logical resource kinds / schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Workspace,
    Tool,
    Fact,
    Observation,
    Inference,
    Artifact,
    Proc,
    Secret,
    Net,
    Actor,
    Trace,
}

impl ResourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Tool => "tool",
            Self::Fact => "fact",
            Self::Observation => "observation",
            Self::Inference => "inference",
            Self::Artifact => "artifact",
            Self::Proc => "proc",
            Self::Secret => "secret",
            Self::Net => "net",
            Self::Actor => "actor",
            Self::Trace => "trace",
        }
    }
}

/// Strongly-typed canonical Resource URI.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceUri(String);

impl ResourceUri {
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CoreError::InvalidUri("Resource URI cannot be empty".into()));
        }

        let (scheme_part, path_part) = trimmed
            .split_once("://")
            .ok_or_else(|| CoreError::InvalidUri(format!("Missing '://' delimiter: '{raw}'")))?;

        let kind = match scheme_part {
            "workspace" => ResourceKind::Workspace,
            "tool" => ResourceKind::Tool,
            "fact" => ResourceKind::Fact,
            "observation" => ResourceKind::Observation,
            "inference" => ResourceKind::Inference,
            "artifact" => ResourceKind::Artifact,
            "proc" => ResourceKind::Proc,
            "secret" => ResourceKind::Secret,
            "net" => ResourceKind::Net,
            "actor" => ResourceKind::Actor,
            "trace" => ResourceKind::Trace,
            other => {
                return Err(CoreError::InvalidUri(format!(
                    "Unknown resource scheme '{other}' in '{raw}'"
                )));
            }
        };

        // Path safety rules: fail closed on traversal or backslashes
        if path_part.contains('\\') {
            return Err(CoreError::InvalidUri(format!(
                "Backslashes are not permitted in logical resource URI: '{raw}'"
            )));
        }

        for segment in path_part.split('/') {
            if segment == ".." {
                return Err(CoreError::InvalidUri(format!(
                    "Directory traversal ('..') is strictly prohibited in resource URI: '{raw}'"
                )));
            }
        }

        if path_part.chars().any(|c| c.is_control()) {
            return Err(CoreError::InvalidUri(format!(
                "Control characters are prohibited in resource URI: '{raw}'"
            )));
        }

        // Scheme specific validations
        if kind == ResourceKind::Artifact {
            if path_part.is_empty() {
                return Err(CoreError::InvalidUri(format!(
                    "Artifact URI requires a path: '{raw}'"
                )));
            }
            if !path_part.starts_with("sha256/") {
                return Err(CoreError::InvalidUri(format!(
                    "Artifact URI must specify 'sha256/<hash>' algorithm: '{raw}'"
                )));
            }
            let hash = &path_part["sha256/".len()..];
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(CoreError::InvalidUri(format!(
                    "Artifact hash must be 64 hexadecimal characters: '{raw}'"
                )));
            }
        }

        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn kind(&self) -> ResourceKind {
        let scheme_part = self.0.split("://").next().unwrap_or("");
        match scheme_part {
            "workspace" => ResourceKind::Workspace,
            "tool" => ResourceKind::Tool,
            "fact" => ResourceKind::Fact,
            "observation" => ResourceKind::Observation,
            "inference" => ResourceKind::Inference,
            "artifact" => ResourceKind::Artifact,
            "proc" => ResourceKind::Proc,
            "secret" => ResourceKind::Secret,
            "net" => ResourceKind::Net,
            "actor" => ResourceKind::Actor,
            "trace" => ResourceKind::Trace,
            _ => unreachable!("Scheme validated during construction"),
        }
    }

    pub fn path(&self) -> &str {
        self.0.split_once("://").map(|(_, p)| p).unwrap_or("")
    }
}

impl fmt::Debug for ResourceUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceUri({})", self.0)
    }
}

impl fmt::Display for ResourceUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ResourceUri {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_uris() {
        assert_eq!(
            ResourceUri::parse("workspace://src/auth.rs")
                .unwrap()
                .kind(),
            ResourceKind::Workspace
        );
        assert_eq!(
            ResourceUri::parse("tool://cargo").unwrap().kind(),
            ResourceKind::Tool
        );
        assert_eq!(
            ResourceUri::parse("fact://git/branch").unwrap().kind(),
            ResourceKind::Fact
        );
        assert_eq!(
            ResourceUri::parse(
                "artifact://sha256/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            )
            .unwrap()
            .kind(),
            ResourceKind::Artifact
        );
    }

    #[test]
    fn traversal_rejected() {
        assert!(ResourceUri::parse("workspace://../etc/passwd").is_err());
        assert!(ResourceUri::parse("workspace://src/../../secret").is_err());
        assert!(ResourceUri::parse("workspace://src/..").is_err());
    }

    #[test]
    fn backslash_rejected() {
        assert!(ResourceUri::parse(r"workspace://src\main.rs").is_err());
    }

    #[test]
    fn malformed_artifacts_rejected() {
        assert!(ResourceUri::parse("artifact://md5/1234").is_err());
        assert!(ResourceUri::parse("artifact://sha256/tooshort").is_err());
        assert!(ResourceUri::parse("artifact://sha256/not_hex_0000000000000000000000000000000000000000000000000000000000000").is_err());
    }
}
