use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Supported logical resource URI schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScheme {
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

impl ResourceScheme {
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
        let (scheme_part, path_part) = raw
            .split_once("://")
            .ok_or_else(|| CoreError::InvalidUri(format!("Missing '://' delimiter: {raw}")))?;

        let scheme = match scheme_part {
            "workspace" => ResourceScheme::Workspace,
            "tool" => ResourceScheme::Tool,
            "fact" => ResourceScheme::Fact,
            "observation" => ResourceScheme::Observation,
            "inference" => ResourceScheme::Inference,
            "artifact" => ResourceScheme::Artifact,
            "proc" => ResourceScheme::Proc,
            "secret" => ResourceScheme::Secret,
            "net" => ResourceScheme::Net,
            "actor" => ResourceScheme::Actor,
            "trace" => ResourceScheme::Trace,
            other => {
                return Err(CoreError::InvalidUri(format!(
                    "Unknown scheme '{other}' in '{raw}'"
                )));
            }
        };

        // Path safety: fail closed on traversal patterns
        if path_part.contains("..") || path_part.contains('\\') {
            return Err(CoreError::InvalidUri(format!(
                "Illegal path traversal or separator in URI: {raw}"
            )));
        }

        // Scheme specific validations
        if scheme == ResourceScheme::Artifact && !path_part.is_empty() {
            if !path_part.starts_with("sha256/") {
                return Err(CoreError::InvalidUri(format!(
                    "Artifact URI must use 'sha256/<hash>' format: {raw}"
                )));
            }
            let hash = &path_part["sha256/".len()..];
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(CoreError::InvalidUri(format!(
                    "Artifact hash must be 64 hex characters: {raw}"
                )));
            }
        }

        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn scheme(&self) -> ResourceScheme {
        let scheme_part = self.0.split("://").next().unwrap_or("");
        match scheme_part {
            "workspace" => ResourceScheme::Workspace,
            "tool" => ResourceScheme::Tool,
            "fact" => ResourceScheme::Fact,
            "observation" => ResourceScheme::Observation,
            "inference" => ResourceScheme::Inference,
            "artifact" => ResourceScheme::Artifact,
            "proc" => ResourceScheme::Proc,
            "secret" => ResourceScheme::Secret,
            "net" => ResourceScheme::Net,
            "actor" => ResourceScheme::Actor,
            "trace" => ResourceScheme::Trace,
            _ => unreachable!(),
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
