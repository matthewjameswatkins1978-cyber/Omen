//! Bounded diagnostic/support bundle (H item 13, 74).
//!
//! Collects version, SHA, contract, ownership, channel, state health,
//! bounded logs, config shape, provider descriptors, doctor results.
//! REDACTS secrets, API keys, auth headers, raw credential values. The
//! bundle is inspectable before sharing: written to a local directory,
//! never uploaded automatically.

use crate::doctor::DoctorReport;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Key names whose values must never appear in a bundle (case-insensitive
/// substring match on the key).
const SECRET_KEY_FRAGMENTS: &[&str] = &[
    "api_key",
    "apikey",
    "token",
    "secret",
    "password",
    "passwd",
    "auth",
    "credential",
    "private_key",
    "privatekey",
    "bearer",
    "session_key",
];

/// Env vars that are safe to record by VALUE (install/shape truth).
const SAFE_ENV_EXACT: &[&str] = &[
    "OMEN_STATE_HOME",
    "OMEN_UPDATE_SOURCE",
    "OMEN_INSTALL_OWNER",
    "OMEN_DEV",
    "NO_COLOR",
    "TERM",
    "COLORTERM",
    "LANG",
    "LC_ALL",
    "WT_SESSION",
];

pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_lowercase();
    SECRET_KEY_FRAGMENTS.iter().any(|f| k.contains(f))
}

/// Redact a JSON value: secret keys become "<redacted>", long strings are
/// bounded. Allowed: credential SOURCE LABELS (key names stay); forbidden:
/// values.
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_secret_key(k) {
                    out.insert(
                        k.clone(),
                        serde_json::Value::String("<redacted>".to_string()),
                    );
                } else {
                    out.insert(k.clone(), redact_json(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_json).collect())
        }
        serde_json::Value::String(s) => {
            // Bound values; scrub bearer-ish tokens even in free text.
            let mut t = s.clone();
            if t.len() > 512 {
                t.truncate(512);
                t.push_str("…<truncated>");
            }
            serde_json::Value::String(scrub_token_text(&t))
        }
        other => other.clone(),
    }
}

fn scrub_token_text(s: &str) -> String {
    // sk-... / ghp_... / xox... shaped values.
    let mut out = s.to_string();
    for prefix in ["sk-", "ghp_", "gho_", "xoxb-", "xoxp-", "Bearer "] {
        let mut start = 0;
        while let Some(idx) = out[start..].find(prefix) {
            let abs = start + idx;
            let end = out[abs..]
                .find(char::is_whitespace)
                .map(|e| abs + e)
                .unwrap_or(out.len());
            let visible = (abs + prefix.len() + 4).min(end);
            out.replace_range(abs..end, &format!("{}<redacted>", &out[abs..visible]));
            start = abs + prefix.len() + 11;
            if start >= out.len() {
                break;
            }
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticBundle {
    pub schema_version: u32,
    pub created_at: String,
    pub version: String,
    pub git_sha: String,
    pub contract_version: String,
    pub ownership: String,
    pub channel: String,
    pub doctor: DoctorReport,
    pub env_shape: serde_json::Value,
    pub config_shape: serde_json::Value,
    pub logs_tail: Vec<String>,
    pub storage_summary: serde_json::Value,
}

/// Collect environment SHAPE: exact values for the safe allowlist, key
/// names only (source labels) for everything else. Values of unknown keys
/// are never recorded.
pub fn collect_env_shape() -> serde_json::Value {
    let mut safe = serde_json::Map::new();
    let mut sources = Vec::new();
    for (k, v) in std::env::vars() {
        if SAFE_ENV_EXACT.contains(&k.as_str()) {
            safe.insert(k, serde_json::Value::String(v));
        } else if is_secret_key(&k) {
            sources.push(format!("{k}=<redacted>"));
        }
    }
    serde_json::json!({"safe": safe, "credential_sources": sources})
}

/// Bounded log tail: last 200 lines of each known log file, if present.
pub fn collect_logs_tail(base: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for name in ["daemon.log", "update/update.log", "install/install.log"] {
        let p = base.join(name);
        if let Ok(text) = std::fs::read_to_string(&p) {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(200);
            out.push(format!(
                "--- {name} (last {} lines) ---",
                lines.len() - start
            ));
            for l in &lines[start..] {
                out.push(bound_line(l));
            }
        }
    }
    out
}

fn bound_line(l: &str) -> String {
    let mut s = l.to_string();
    if s.len() > 500 {
        s.truncate(500);
        s.push_str("…<truncated>");
    }
    scrub_token_text(&s)
}

/// Write the bundle to `<base>/evidence/diag-<stamp>/` and return the dir.
/// Inspectable before sharing; nothing is transmitted.
pub fn write_bundle(
    base: &Path,
    bundle: &DiagnosticBundle,
) -> Result<PathBuf, crate::error::LifecycleError> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dir = base.join("evidence").join(format!("diag-{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::LifecycleError::Io(e.to_string()))?;
    let redacted = redact_json(&serde_json::to_value(bundle).unwrap());
    std::fs::write(
        dir.join("bundle.json"),
        serde_json::to_vec_pretty(&redacted).unwrap(),
    )
    .map_err(|e| crate::error::LifecycleError::Io(e.to_string()))?;
    std::fs::write(
        dir.join("README.txt"),
        "Omen diagnostic bundle. Inspect bundle.json before sharing. Omen never uploads this automatically.\n",
    )
    .map_err(|e| crate::error::LifecycleError::Io(e.to_string()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_redacted_values_labels_kept() {
        let v = serde_json::json!({
            "openai_api_key": "sk-live-SECRETVALUE",
            "provider": "codex",
            "nested": {"token": "abc", "count": 3},
            "free": "hello sk-abcdefg world"
        });
        let r = redact_json(&v);
        assert_eq!(r["openai_api_key"], "<redacted>");
        assert_eq!(r["provider"], "codex");
        assert_eq!(r["nested"]["token"], "<redacted>");
        assert_eq!(r["nested"]["count"], 3);
        assert!(r["free"].as_str().unwrap().contains("<redacted>"));
        assert!(!r["free"].as_str().unwrap().contains("abcdefg"));
    }

    #[test]
    fn env_shape_never_records_unknown_values() {
        // SAFETY: test-only vars with unique names; single-threaded test.
        unsafe {
            std::env::set_var("OMEN_DIAG_TEST_SAFE", "visible-but-unknown-key");
            std::env::set_var("OMEN_DIAG_TEST_SYNTHETIC_SECRET", "synth-value-123");
        }
        let shape = collect_env_shape();
        let text = serde_json::to_string(&shape).unwrap();
        assert!(!text.contains("synth-value-123"));
        assert!(!text.contains("visible-but-unknown-key"));
        unsafe {
            std::env::remove_var("OMEN_DIAG_TEST_SAFE");
            std::env::remove_var("OMEN_DIAG_TEST_SYNTHETIC_SECRET");
        }
    }
}
