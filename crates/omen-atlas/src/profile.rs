use omen_core::CoreError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProbeConfig {
    #[serde(default)]
    pub version_args: Vec<String>,
    #[serde(default)]
    pub health_args: Vec<String>,
    #[serde(default)]
    pub capabilities_args: Vec<String>,
}

/// Tool runtime profile describing CLI hints and probe configurations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeProfile {
    pub tool_id: String,
    pub binary_name: String,
    pub description: String,
    pub understanding: String,
    pub stdin_mode: String,
    #[serde(default)]
    pub probes: ProbeConfig,
    #[serde(default)]
    pub structured_output: Option<toml::Table>,
}

impl RuntimeProfile {
    pub fn from_file(path: &Path) -> Result<Self, CoreError> {
        let content = fs::read_to_string(path)
            .map_err(|e| CoreError::NotFound(format!("Failed to read profile {:?}: {e}", path)))?;
        Self::from_toml_str(&content)
    }

    pub fn from_toml_str(content: &str) -> Result<Self, CoreError> {
        let profile: Self = toml::from_str(content).map_err(|e| {
            CoreError::SchemaViolation(format!("Failed to parse profile TOML: {e}"))
        })?;
        Ok(profile)
    }
}

impl FromStr for RuntimeProfile {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_toml_str(s)
    }
}
