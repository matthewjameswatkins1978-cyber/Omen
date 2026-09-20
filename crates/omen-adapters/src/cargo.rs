use omen_core::{Assurance, CoreError, RequiredAssurance, ResourceUri, RetentionClass, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use omen_knowledge::{
    ContentAddressedStore, Database, FactRecord, FactRegistry, PublishFactRequest,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilerDiagnosticSpan {
    pub file_name: String,
    pub line_start: usize,
    pub line_end: usize,
    pub column_start: usize,
    pub column_end: usize,
    pub is_primary: bool,
    pub text: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilerDiagnostic {
    pub message: String,
    pub level: String,
    pub code: Option<String>,
    pub spans: Vec<CompilerDiagnosticSpan>,
    pub rendered: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoCheckResult {
    pub success: bool,
    pub diagnostics: Vec<CompilerDiagnostic>,
    pub artifact_uri: ResourceUri,
    pub compiler_fact: FactRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoTestResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub artifact_uri: ResourceUri,
    pub test_fact: FactRecord,
}

pub struct CargoAdapter;

impl CargoAdapter {
    /// Run `cargo metadata --format-version 1 --no-deps` and parse workspace/package info
    pub async fn metadata(
        supervisor: &ProcessSupervisor,
        manifest_dir: &Path,
    ) -> Result<serde_json::Value, CoreError> {
        let req = ExecutionRequest {
            argv: vec![
                "cargo".into(),
                "metadata".into(),
                "--format-version".into(),
                "1".into(),
                "--no-deps".into(),
            ],
            cwd: manifest_dir.to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 15000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = supervisor.execute(req).await?;
        let text = String::from_utf8_lossy(&output.stdout_all);
        serde_json::from_str(&text).map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to parse cargo metadata JSON: {e}"))
        })
    }

    /// Run `cargo check --message-format=json` and publish `fact://compiler/errors`
    pub async fn check(
        supervisor: &ProcessSupervisor,
        cas: &ContentAddressedStore,
        db: &mut Database,
        manifest_dir: &Path,
        package: Option<&str>,
    ) -> Result<CargoCheckResult, CoreError> {
        let mut argv: Vec<String> = vec![
            "cargo".into(),
            "check".into(),
            "--message-format=json".into(),
        ];
        if let Some(pkg) = package {
            argv.push("-p".into());
            argv.push(pkg.into());
        }

        let req = ExecutionRequest {
            argv,
            cwd: manifest_dir.to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 60000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = supervisor.execute(req).await?;

        // Spool full compilation stream into CAS
        let artifact = cas.store(
            db,
            &output.stdout_all,
            "application/x-ndjson",
            "omen://adapter/cargo/check",
            RetentionClass::Referenced,
        )?;

        let raw = String::from_utf8_lossy(&output.stdout_all);
        let mut diagnostics = Vec::new();
        let mut has_error = false;

        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                let is_compiler_msg =
                    val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message");
                if is_compiler_msg && let Some(msg_val) = val.get("message") {
                    let level = msg_val
                        .get("level")
                        .and_then(|l| l.as_str())
                        .unwrap_or("unknown");
                    if level == "error" {
                        has_error = true;
                    }
                    if let Ok(diag) = serde_json::from_value::<CompilerDiagnostic>(msg_val.clone())
                    {
                        diagnostics.push(diag);
                    }
                }
            }
        }

        let success = output.process_exit.is_zero() && !has_error;
        let fact_resource = ResourceUri::parse("fact://compiler/errors")?;
        let current_gen = FactRegistry::get_generation(db, "fs:workspace")?;

        let value = if success {
            "none".to_string()
        } else {
            let err_count = diagnostics.iter().filter(|d| d.level == "error").count();
            format!("errors_found: {}", err_count)
        };

        let fact_req = PublishFactRequest {
            resource: &fact_resource,
            value: &value,
            assurance: Assurance::Verified,
            producer: "omen://adapter/cargo",
            witness: Some(&format!("cargo-check exit: {:?}", output.process_exit.code)),
            dependencies: &[("fs:workspace".into(), current_gen)],
            artifacts: std::slice::from_ref(&artifact.uri),
        };

        let compiler_fact = FactRegistry::publish_fact(db, fact_req)?;

        Ok(CargoCheckResult {
            success,
            diagnostics,
            artifact_uri: artifact.uri,
            compiler_fact,
        })
    }

    /// Run `cargo test` and publish `fact://test/status` (clean separation from compiler diagnostics)
    pub async fn test(
        supervisor: &ProcessSupervisor,
        cas: &ContentAddressedStore,
        db: &mut Database,
        manifest_dir: &Path,
        package: Option<&str>,
        test_name: Option<&str>,
    ) -> Result<CargoTestResult, CoreError> {
        let mut argv: Vec<String> = vec!["cargo".into(), "test".into()];
        if let Some(pkg) = package {
            argv.push("-p".into());
            argv.push(pkg.into());
        }
        if let Some(t) = test_name {
            argv.push("--test".into());
            argv.push(t.into());
        }

        let req = ExecutionRequest {
            argv,
            cwd: manifest_dir.to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 120000,
            inline_budget: 65536,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = supervisor.execute(req).await?;

        // Combine stdout + stderr for test output artifact
        let mut combined_output = Vec::new();
        combined_output.extend_from_slice(&output.stdout_all);
        combined_output.extend_from_slice(b"\n--- STDERR ---\n");
        combined_output.extend_from_slice(&output.stderr_all);

        let artifact = cas.store(
            db,
            &combined_output,
            "text/plain",
            "omen://adapter/cargo/test",
            RetentionClass::Referenced,
        )?;

        let success = output.process_exit.is_zero();
        let fact_resource = ResourceUri::parse("fact://test/status")?;
        let current_gen = FactRegistry::get_generation(db, "fs:workspace")?;

        let value = if success { "passing" } else { "failing" };

        let fact_req = PublishFactRequest {
            resource: &fact_resource,
            value,
            assurance: Assurance::Verified,
            producer: "omen://adapter/cargo",
            witness: Some(&format!("cargo-test exit: {:?}", output.process_exit.code)),
            dependencies: &[("fs:workspace".into(), current_gen)],
            artifacts: std::slice::from_ref(&artifact.uri),
        };

        let test_fact = FactRegistry::publish_fact(db, fact_req)?;

        Ok(CargoTestResult {
            success,
            exit_code: output.process_exit.code,
            artifact_uri: artifact.uri,
            test_fact,
        })
    }
}
