use omen_core::{CoreError, RequiredAssurance, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tempfile::NamedTempFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothByteRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothCertificate {
    pub protocol_version: Option<String>,
    pub outcome: String,
    pub file_path: String,
    #[serde(default)]
    pub pre_hash: Option<String>,
    #[serde(default)]
    pub post_hash: Option<String>,
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub diff_summary: Option<String>,
    #[serde(default)]
    pub changed_ranges: Vec<ThreadMothByteRange>,
}

impl ThreadMothCertificate {
    pub fn is_applied(&self) -> bool {
        self.outcome == "APPLIED"
    }

    pub fn is_refused(&self) -> bool {
        self.outcome == "REFUSED"
    }
}

/// Canonical ThreadMoth request model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothRequest {
    pub version: String,
    pub request_id: String,
    pub file_path: String,
    pub namespace: ThreadMothNamespace,
    pub cardinality: ThreadMothCardinality,
    pub operation: ThreadMothOperationWrapper,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_pre_hash: Option<String>,
    pub budget: ThreadMothBudget,
    pub allow_generated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothNamespace {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothCardinality {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothOperationWrapper {
    pub provider: String,
    pub operation: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMothBudget {
    pub max_files: usize,
    pub max_matches: usize,
    pub max_changed_regions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_changed_bytes: Option<usize>,
    pub allowed_path_prefixes: Vec<String>,
}

pub struct ThreadMothAdapter;

impl ThreadMothAdapter {
    pub async fn doctor(supervisor: &ProcessSupervisor) -> Result<serde_json::Value, CoreError> {
        let req = ExecutionRequest {
            argv: vec!["threadmoth".into(), "doctor".into(), "--json".into()],
            cwd: std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 5000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
        };

        let output = supervisor.execute(req).await?;
        let text = String::from_utf8_lossy(&output.stdout_all);
        serde_json::from_str(&text)
            .map_err(|e| CoreError::ExecutionFailed(format!("Failed to parse doctor JSON: {e}")))
    }

    pub async fn capabilities(
        supervisor: &ProcessSupervisor,
    ) -> Result<serde_json::Value, CoreError> {
        let req = ExecutionRequest {
            argv: vec!["threadmoth".into(), "capabilities".into(), "--json".into()],
            cwd: std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 5000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
        };

        let output = supervisor.execute(req).await?;
        let text = String::from_utf8_lossy(&output.stdout_all);
        serde_json::from_str(&text).map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to parse capabilities JSON: {e}"))
        })
    }

    pub async fn mutate_request(
        supervisor: &ProcessSupervisor,
        cwd: &Path,
        request: &ThreadMothRequest,
    ) -> Result<ThreadMothCertificate, CoreError> {
        let req_file = NamedTempFile::new()
            .map_err(|e| CoreError::Internal(format!("Failed to create temp request file: {e}")))?;

        let serialized = serde_json::to_string_pretty(request).map_err(|e| {
            CoreError::SchemaViolation(format!("Failed to serialize ThreadMoth request: {e}"))
        })?;

        fs::write(req_file.path(), serialized.as_bytes())
            .map_err(|e| CoreError::Internal(format!("Failed to write ThreadMoth request: {e}")))?;

        let req = ExecutionRequest {
            argv: vec![
                "threadmoth".into(),
                "mutate".into(),
                "--request".into(),
                req_file.path().to_string_lossy().to_string(),
            ],
            cwd: cwd.to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 10000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
        };

        let output = supervisor.execute(req).await?;
        let raw = String::from_utf8_lossy(&output.stdout_all);

        // ThreadMoth outputs JSON certificate on stdout
        let cert: ThreadMothCertificate = serde_json::from_str(&raw).map_err(|e| {
            CoreError::ExecutionFailed(format!(
                "Failed to parse ThreadMoth certificate: {e}. Output was: {raw}"
            ))
        })?;

        Ok(cert)
    }

    /// Convenience wrapper for exact text replacement using canonical request format
    pub async fn replace_exact(
        supervisor: &ProcessSupervisor,
        cwd: &Path,
        relative_path: &str,
        target: &str,
        replacement: &str,
        expected_pre_hash: Option<&str>,
    ) -> Result<ThreadMothCertificate, CoreError> {
        let norm_path = relative_path.replace('\\', "/");
        let request = ThreadMothRequest {
            version: "1.3.1".into(),
            request_id: format!("req-{}", norm_path),
            file_path: norm_path,
            namespace: ThreadMothNamespace {
                kind: "native".into(),
            },
            cardinality: ThreadMothCardinality {
                kind: "exactly_one".into(),
            },
            operation: ThreadMothOperationWrapper {
                provider: "text".into(),
                operation: serde_json::json!({
                    "type": "replace",
                    "target": target,
                    "replacement": replacement,
                }),
            },
            expected_pre_hash: expected_pre_hash.map(Into::into),
            budget: ThreadMothBudget {
                max_files: 1,
                max_matches: 1,
                max_changed_regions: 1,
                max_changed_bytes: None,
                allowed_path_prefixes: vec![],
            },
            allow_generated: false,
        };

        Self::mutate_request(supervisor, cwd, &request).await
    }
}
