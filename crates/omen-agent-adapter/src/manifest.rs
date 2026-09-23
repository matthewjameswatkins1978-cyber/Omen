//! Adapter package manifest: rigid, bounded, authority-free.
//!
//! A manifest declares what an adapter NEEDS. It never declares what the
//! adapter is ALLOWED. Tethers / Omen admission remains authoritative.
//! Unknown manifest fields are preserved (bounded) and semantically inert.

use serde::{Deserialize, Serialize};

/// Manifest schema version (distinct from adapter version and protocol version).
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_LIST_ENTRIES: usize = 64;
pub const MAX_STRING_LEN: usize = 1024;

/// Portable adapter package description.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterManifest {
    pub schema_version: u32,
    pub adapter_id: String,
    pub adapter_version: String,
    /// argv entrypoint template. Never a shell string; index 0 is resolved
    /// against PATH at bind time and recorded by digest.
    pub entrypoint: Vec<String>,
    pub supported_protocol_versions: Vec<String>,
    pub required_omen_contracts: Vec<String>,
    pub capabilities: Vec<String>,
    pub required_programs: Vec<String>,
    pub required_config_labels: Vec<String>,
    /// Credential SOURCE labels only. Values are forbidden.
    pub credential_labels: Vec<String>,
    pub transports: Vec<String>,
    pub streaming: bool,
    pub platforms: Vec<String>,
    /// Optional features the adapter can live without (degrade truthfully).
    #[serde(default)]
    pub optional_features: Vec<String>,
    /// Required features; absence is explicit refusal, never reinterpretation.
    #[serde(default)]
    pub required_features: Vec<String>,
    /// Unknown fields: bounded, preserved, zero authority effect.
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl AdapterManifest {
    /// Validates rigidity: sizes, argv shape, forbidden authority content.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema {
                got: self.schema_version,
            });
        }
        check_len("adapter_id", &self.adapter_id)?;
        check_len("adapter_version", &self.adapter_version)?;
        if !is_valid_id(&self.adapter_id) {
            return Err(ManifestError::BadId(self.adapter_id.clone()));
        }
        if self.entrypoint.is_empty() || self.entrypoint.len() > 16 {
            return Err(ManifestError::BadEntrypoint(
                "entrypoint must be 1..=16 argv elements".into(),
            ));
        }
        for arg in &self.entrypoint {
            check_len("entrypoint_arg", arg)?;
            // argv elements are literal: no shell metacharacters, no nesting.
            if arg.contains(['|', '&', ';', '$', '`', '\n', '\r', '"', '\''])
                || arg.trim_start().starts_with("sh ")
                || arg.trim_start().starts_with("cmd ")
            {
                return Err(ManifestError::BadEntrypoint(format!(
                    "shell syntax forbidden in argv element: {arg:?}"
                )));
            }
        }
        if self.supported_protocol_versions.is_empty()
            || self.supported_protocol_versions.len() > MAX_LIST_ENTRIES
        {
            return Err(ManifestError::BadList("supported_protocol_versions".into()));
        }
        if self.required_omen_contracts.is_empty() {
            return Err(ManifestError::BadList("required_omen_contracts".into()));
        }
        check_list("capabilities", &self.capabilities)?;
        check_list("required_programs", &self.required_programs)?;
        check_list("required_config_labels", &self.required_config_labels)?;
        check_list("credential_labels", &self.credential_labels)?;
        check_list("transports", &self.transports)?;
        check_list("platforms", &self.platforms)?;
        if self.extra.len() > 32 {
            return Err(ManifestError::TooManyUnknownFields {
                count: self.extra.len(),
            });
        }
        // Forbidden: anything shaped like granted authority or live secrets.
        for key in self.extra.keys() {
            let lower = key.to_lowercase();
            if lower.contains("permission")
                || lower.contains("allow_")
                || lower.contains("grant")
                || lower.contains("authority")
                || lower.contains("token")
                || lower.contains("secret")
                || lower.contains("api_key")
                || lower.contains("apikey")
            {
                return Err(ManifestError::ForbiddenField(key.clone()));
            }
        }
        for label in &self.credential_labels {
            check_len("credential_label", label)?;
            if looks_like_secret_value(label) {
                return Err(ManifestError::SecretValueInManifest);
            }
        }
        Ok(())
    }
}

fn check_len(field: &str, value: &str) -> Result<(), ManifestError> {
    if value.is_empty() || value.len() > MAX_STRING_LEN {
        return Err(ManifestError::BadString {
            field: field.into(),
        });
    }
    Ok(())
}

fn check_list(field: &str, list: &[String]) -> Result<(), ManifestError> {
    if list.len() > MAX_LIST_ENTRIES {
        return Err(ManifestError::BadList(field.into()));
    }
    for item in list {
        check_len(field, item)?;
    }
    Ok(())
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn looks_like_secret_value(label: &str) -> bool {
    // Labels are short names like "codex-auth"; values are long/opaque.
    label.len() > 96 || label.contains(' ') || label.contains('\n')
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("unsupported manifest schema version: {got}")]
    UnsupportedSchema { got: u32 },
    #[error("bad adapter id: {0:?}")]
    BadId(String),
    #[error("bad entrypoint: {0}")]
    BadEntrypoint(String),
    #[error("bad list '{0}'")]
    BadList(String),
    #[error("bad string field '{field}'")]
    BadString { field: String },
    #[error("too many unknown manifest fields: {count}")]
    TooManyUnknownFields { count: usize },
    #[error("forbidden manifest field (authority/secret shaped): {0:?}")]
    ForbiddenField(String),
    #[error("credential labels must be labels, never secret values")]
    SecretValueInManifest,
}
