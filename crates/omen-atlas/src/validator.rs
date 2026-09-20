use crate::tool::{ToolInstance, ValidationState};
use chrono::Utc;
use omen_core::{CoreError, RequiredAssurance, StdioMode};
use omen_engine::{ExecutionRequest, ProcessSupervisor};
use tempfile::tempdir;

pub struct ToolValidator {
    supervisor: ProcessSupervisor,
}

impl ToolValidator {
    pub fn new() -> Self {
        Self {
            supervisor: ProcessSupervisor::new(),
        }
    }

    pub async fn validate(&self, tool: &mut ToolInstance) -> Result<(), CoreError> {
        let fixture_dir = tempdir().map_err(|e| {
            CoreError::Internal(format!("Failed to create validation fixture: {e}"))
        })?;

        let version_args = if let Some(profile) = &tool.profile {
            if !profile.probes.version_args.is_empty() {
                profile.probes.version_args.clone()
            } else {
                vec!["--version".into()]
            }
        } else {
            vec!["--version".into()]
        };

        let mut argv = vec![tool.binary_path.to_string_lossy().to_string()];
        argv.extend(version_args);

        let req = ExecutionRequest {
            argv,
            cwd: fixture_dir.path().to_path_buf(),
            env: vec![],
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 5000,
            inline_budget: 4096,
            required_assurance: RequiredAssurance::default(),
            secrets: vec![],
        };

        let output = self.supervisor.execute(req).await?;

        if output.runtime_status == omen_core::RuntimeStatus::Completed
            && output.process_exit.is_zero()
        {
            let raw_stdout = String::from_utf8_lossy(&output.stdout_bounded);
            let first_line = raw_stdout.lines().next().unwrap_or("").trim();

            tool.version = Some(first_line.to_string());
            tool.validation_state = ValidationState::Validated;
            tool.last_validated_at = Some(Utc::now().to_rfc3339());
            Ok(())
        } else {
            tool.validation_state = ValidationState::Incompatible;
            Err(CoreError::ExecutionFailed(format!(
                "Tool validation probe failed with exit {:?}",
                output.process_exit.code
            )))
        }
    }
}

impl Default for ToolValidator {
    fn default() -> Self {
        Self::new()
    }
}
