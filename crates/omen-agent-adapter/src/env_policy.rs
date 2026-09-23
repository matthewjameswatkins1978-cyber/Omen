//! Explicit environment policy. Third-party adapters receive only
//! what they genuinely require. The full Omen environment is never
//! inherited blindly. Credential values never cross; labels may.

use serde::{Deserialize, Serialize};

/// Allowlist policy for adapter child environments.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvPolicy {
    /// Exact variable names passed through when present in the parent env.
    pub allow_vars: Vec<String>,
    /// Literal variables injected by Omen (never secrets).
    pub set_vars: Vec<(String, String)>,
}

impl EnvPolicy {
    /// Builds the child environment: allowlisted passthrough + Omen-set
    /// literals. Everything else — including OPENAI_API_KEY unless the
    /// adapter manifest explicitly requires its label — is dropped.
    pub fn build_env(
        &self,
        parent: &[(String, String)],
        required_credential_labels: &[String],
    ) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (k, v) in parent {
            if self.allow_vars.iter().any(|a| a == k) {
                out.push((k.clone(), v.clone()));
            }
        }
        // Credential VALUES are never injected from labels. A label only
        // documents which external authenticator the adapter will consult
        // itself (e.g. Codex reads its own CODEX_HOME store).
        let _ = required_credential_labels;
        for (k, v) in &self.set_vars {
            if let Some(slot) = out.iter_mut().find(|(ek, _)| ek == k) {
                slot.1 = v.clone();
            } else {
                out.push((k.clone(), v.clone()));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Audit helper: asserts none of the given secret values appear in the
    /// built environment. Used by tests with synthetic secrets.
    pub fn assert_no_secret_leak(env: &[(String, String)], secrets: &[&str]) -> Result<(), String> {
        for (k, v) in env {
            for secret in secrets {
                if !secret.is_empty() && v.contains(secret) {
                    return Err(format!("secret leaked into adapter env var {k:?}"));
                }
            }
        }
        Ok(())
    }
}

/// Minimal environment genuinely required by the Codex reference route.
///
/// Why each entry exists:
/// - `SystemRoot`, `SystemDrive`: Windows loader/sandbox helpers.
/// - `USERPROFILE`: Codex locates its own CODEX_HOME auth store beneath it.
/// - `CODEX_HOME`: honored when the user relocated the Codex store.
/// - `TEMP`, `TMP`: Codex scratch + Omen schema files.
/// - `PATH`: resolving Codex's own bundled helpers (rg, sandbox setup).
/// - `NO_COLOR`: keep machine output free of ANSI decoration.
///
/// Conspicuously absent: OPENAI_API_KEY and every other credential value.
pub fn codex_env_policy() -> EnvPolicy {
    EnvPolicy {
        allow_vars: vec![
            "SystemRoot".into(),
            "SystemDrive".into(),
            "USERPROFILE".into(),
            "CODEX_HOME".into(),
            "TEMP".into(),
            "TMP".into(),
            "PATH".into(),
            "NO_COLOR".into(),
        ],
        set_vars: vec![("NO_COLOR".into(), "1".into())],
    }
}

/// Minimal environment for the deterministic fixture adapter (tests only).
pub fn fixture_env_policy() -> EnvPolicy {
    EnvPolicy {
        allow_vars: vec![
            "SystemRoot".into(),
            "SystemDrive".into(),
            "TEMP".into(),
            "TMP".into(),
            "PATH".into(),
            "OMB_FIXTURE_MODE".into(),
            "OMB_FIXTURE_COUNT_FILE".into(),
            "OMB_FIXTURE_SENTINEL".into(),
        ],
        set_vars: vec![],
    }
}
