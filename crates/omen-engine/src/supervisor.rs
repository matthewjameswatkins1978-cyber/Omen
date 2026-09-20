use crate::backend::{ExecutionBackend, NativeExecutionBackend};
use omen_core::{
    AdapterClassification, CoreError, EnforcementReport, ProcessExit, RequiredAssurance,
    RuntimeStatus, SecretInjectionContract, StdioMode,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_INLINE_BUDGET: usize = 8192;

/// A secret injected into an execution request with a strict injection contract.
#[derive(Debug, Clone)]
pub struct ExecutionSecret {
    pub name: String,
    pub value: String,
    pub contract: SecretInjectionContract,
}

impl ExecutionSecret {
    pub fn env(name: impl Into<String>, value: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            name: n.clone(),
            value: value.into(),
            contract: SecretInjectionContract::EnvironmentVariable { name: n },
        }
    }

    pub fn stdin(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            contract: SecretInjectionContract::Stdin,
        }
    }

    pub fn temp_file(
        name: impl Into<String>,
        value: impl Into<String>,
        file_name: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            contract: SecretInjectionContract::TemporaryFile { file_name },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub stdin_mode: StdioMode,
    pub stdin_payload: Option<Vec<u8>>,
    pub timeout_ms: u64,
    pub inline_budget: usize,
    pub required_assurance: RequiredAssurance,
    pub secrets: Vec<ExecutionSecret>,
}

impl ExecutionRequest {
    pub fn simple(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            argv,
            cwd,
            env: Vec::new(),
            stdin_mode: StdioMode::Closed,
            stdin_payload: None,
            timeout_ms: 10000,
            inline_budget: DEFAULT_INLINE_BUDGET,
            required_assurance: RequiredAssurance::default(),
            secrets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    pub runtime_status: RuntimeStatus,
    pub process_exit: ProcessExit,
    pub adapter_classification: AdapterClassification,
    pub enforcement: EnforcementReport,
    pub stdout_bounded: Vec<u8>,
    pub stderr_bounded: Vec<u8>,
    pub stdout_all: Vec<u8>,
    pub stderr_all: Vec<u8>,
    pub duration_ms: u64,
}

pub struct ProcessSupervisor {
    backend: Arc<dyn ExecutionBackend>,
}

impl ProcessSupervisor {
    pub fn new() -> Self {
        Self {
            backend: Arc::new(NativeExecutionBackend::new()),
        }
    }

    pub fn with_backend(backend: Arc<dyn ExecutionBackend>) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &dyn ExecutionBackend {
        self.backend.as_ref()
    }

    pub async fn execute(&self, mut req: ExecutionRequest) -> Result<ExecutionOutput, CoreError> {
        // 1. Preflight check: fail-closed if required assurance cannot be enforced
        self.backend.preflight(&req.required_assurance)?;

        if req.argv.is_empty() {
            return Err(CoreError::ExecutionFailed("Argv cannot be empty".into()));
        }

        let start_time = std::time::Instant::now();

        // Handle temporary file secret injection
        let _temp_dir_guard = if req
            .secrets
            .iter()
            .any(|s| matches!(s.contract, SecretInjectionContract::TemporaryFile { .. }))
        {
            let tmp = tempfile::tempdir().map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to create secret temp dir: {e}"))
            })?;
            for secret in &req.secrets {
                if let SecretInjectionContract::TemporaryFile { file_name } = &secret.contract {
                    let fname = file_name.as_deref().unwrap_or("secret.token");
                    let fpath = tmp.path().join(fname);
                    std::fs::write(&fpath, &secret.value).map_err(|e| {
                        CoreError::ExecutionFailed(format!("Failed to write secret file: {e}"))
                    })?;
                    req.env.push((
                        format!("{}_FILE", secret.name),
                        fpath.to_string_lossy().to_string(),
                    ));
                }
            }
            Some(tmp)
        } else {
            None
        };

        // 2. Physical spawn through the execution backend
        let handle = self.backend.spawn(&req)?;

        // 3. Supervised wait with bounded timeout and tree cleanup
        let timeout_duration = Duration::from_millis(req.timeout_ms);
        let (runtime_status, process_exit, mut stdout_all, mut stderr_all) =
            handle.wait_bounded(timeout_duration).await?;

        // 4. Automatic secret redaction from captured stdout and stderr
        for secret in &req.secrets {
            if !secret.value.is_empty() {
                let needle = secret.value.as_bytes();
                let replacement = format!("[REDACTED:{}]", secret.name).into_bytes();
                stdout_all = redact_bytes(&stdout_all, needle, &replacement);
                stderr_all = redact_bytes(&stderr_all, needle, &replacement);
            }
        }

        // 5. Bounded context slices
        let stdout_bounded = if stdout_all.len() > req.inline_budget {
            stdout_all[..req.inline_budget].to_vec()
        } else {
            stdout_all.clone()
        };

        let stderr_bounded = if stderr_all.len() > req.inline_budget {
            stderr_all[..req.inline_budget].to_vec()
        } else {
            stderr_all.clone()
        };

        let adapter_classification = if runtime_status == RuntimeStatus::Completed {
            if process_exit.is_zero() {
                AdapterClassification::Success
            } else {
                AdapterClassification::Failure
            }
        } else {
            AdapterClassification::Failure
        };

        let caps = self.backend.capabilities();
        let enforcement = EnforcementReport {
            filesystem: caps.filesystem,
            network: caps.network,
            descendant_processes: caps.descendants,
            symlink_escape: caps.symlink_escape,
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(ExecutionOutput {
            runtime_status,
            process_exit,
            adapter_classification,
            enforcement,
            stdout_bounded,
            stderr_bounded,
            stdout_all,
            stderr_all,
            duration_ms,
        })
    }
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

fn redact_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() || haystack.is_empty() || haystack.len() < needle.len() {
        return haystack.to_vec();
    }
    let mut result = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i <= haystack.len().saturating_sub(needle.len()) {
        if &haystack[i..i + needle.len()] == needle {
            result.extend_from_slice(replacement);
            i += needle.len();
        } else {
            result.push(haystack[i]);
            i += 1;
        }
    }
    if i < haystack.len() {
        result.extend_from_slice(&haystack[i..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_bytes() {
        let original = b"LOG: token=OMEN_SECRET_CANARY_XYZ12345 in stream";
        let redacted = redact_bytes(
            original,
            b"OMEN_SECRET_CANARY_XYZ12345",
            b"[REDACTED:token]",
        );
        assert_eq!(
            String::from_utf8(redacted).unwrap(),
            "LOG: token=[REDACTED:token] in stream"
        );
    }
}
