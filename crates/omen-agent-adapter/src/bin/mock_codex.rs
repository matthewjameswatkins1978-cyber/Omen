//! Deterministic stand-in for `codex exec --json` (CI only).
//!
//! Selected by `OMB_MOCK_CODEX_TRANSCRIPT`. Emits canned JSONL event
//! streams shaped like real Codex machine output so the Codex driver
//! translation is testable without credentials, network, or credits.
//! Counts spawns by appending to `OMB_MOCK_CODEX_COUNT_FILE` when set.

use std::io::Write;

fn main() {
    // Like the real binary, --version is auth-free and transcript-free,
    // except two test-only modes selected via the transcript knob:
    // "version-stall" sleeps past any probe deadline (timeout test), and
    // every --version records an env snapshot to a fixed temp path so probe
    // isolation tests can audit the child environment deterministically.
    if std::env::args().any(|a| a == "--version") {
        if std::env::var("OMB_MOCK_CODEX_TRANSCRIPT").as_deref() == Ok("version-stall") {
            let _ = std::fs::write(version_pid_path(), std::process::id().to_string());
            std::thread::sleep(std::time::Duration::from_secs(120));
            return;
        }
        record_version_env();
        println!("mock-codex 0.0.0");
        return;
    }
    let spawn_index = count_file_lines() + 1;
    if let Ok(count_file) = std::env::var("OMB_MOCK_CODEX_COUNT_FILE")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&count_file)
    {
        let _ = writeln!(f, "spawn");
    }
    let transcript =
        std::env::var("OMB_MOCK_CODEX_TRANSCRIPT").unwrap_or_else(|_| "success".into());
    match transcript.as_str() {
        "success" => {
            emit_success(
                &std::env::var("OMB_MOCK_CODEX_MESSAGE")
                    .unwrap_or_else(|_| r#"{"message":"mock codex explanation"}"#.into()),
            );
        }
        // First spawn requests a tool, later spawns answer finally.
        "tool-roundtrip" => {
            if spawn_index <= 1 {
                emit_success(&std::env::var("OMB_MOCK_CODEX_MESSAGE").unwrap_or_else(|_| {
                    r#"{"message":"requesting status","tool_requests":[{"tool":"omen.workspace_status","operation":"status","input":{}}]}"#.into()
                }));
            } else {
                emit_success(
                    &std::env::var("OMB_MOCK_CODEX_MESSAGE_2").unwrap_or_else(|_| {
                        r#"{"message":"final answer incorporating tool result"}"#.into()
                    }),
                );
            }
        }
        // Every spawn requests a tool: the driver must refuse the loop.
        "tool-twice" => {
            emit_success(&std::env::var("OMB_MOCK_CODEX_MESSAGE").unwrap_or_else(|_| {
                r#"{"message":"requesting again","tool_requests":[{"tool":"omen.workspace_status","operation":"status","input":{}}]}"#.into()
            }));
        }
        "malformed" => {
            event(r#"{"type":"thread.started","thread_id":"mock-thread-1"}"#);
            println!("THIS IS NOT JSONL {{{{");
        }
        "auth-fail" => {
            eprintln!("Not logged in. Run `codex login` to authenticate.");
            std::process::exit(1);
        }
        "rate-limited" => {
            event(r#"{"type":"thread.started","thread_id":"mock-thread-1"}"#);
            event(r#"{"type":"turn.started"}"#);
            event(
                r#"{"type":"error","message":"{\"type\":\"error\",\"status\":429,\"error\":{\"message\":\"rate limited\"}}"}"#,
            );
            event(r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#);
            std::process::exit(1);
        }
        "stall" => {
            event(r#"{"type":"thread.started","thread_id":"mock-thread-1"}"#);
            std::thread::sleep(std::time::Duration::from_secs(120));
        }
        "crash" => {
            event(r#"{"type":"thread.started","thread_id":"mock-thread-1"}"#);
            event(r#"{"type":"turn.started"}"#);
            std::process::exit(3);
        }
        "empty" => {
            event(r#"{"type":"thread.started","thread_id":"mock-thread-1"}"#);
            event(r#"{"type":"turn.started"}"#);
            event(r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":0}}"#);
        }
        other => {
            eprintln!("unknown mock transcript: {other}");
            std::process::exit(2);
        }
    }
}

/// Fixed temp path where `--version` records the child environment it
/// actually received. Fixed (not relayed via env) because the probe under
/// test only forwards the production allowlist — the control knob could
/// never arrive any other way.
fn version_env_path() -> std::path::PathBuf {
    std::env::temp_dir().join("omen-mock-codex-version-env.txt")
}

fn version_pid_path() -> std::path::PathBuf {
    std::env::temp_dir().join("omen-mock-codex-version-pid.txt")
}

/// Records secret-shaped keys (must be absent under the probe policy) and
/// approved keys (must arrive) for the probe isolation test to audit.
fn record_version_env() {
    let mut lines = Vec::new();
    for key in [
        "OPENAI_API_KEY",
        "OMEN_SYNTHETIC_SECRET",
        "PATH",
        "SystemRoot",
        "USERPROFILE",
        "TEMP",
        "NO_COLOR",
    ] {
        match std::env::var(key) {
            Ok(v) => lines.push(format!("{key}={v}")),
            Err(_) => lines.push(format!("{key}=MISSING")),
        }
    }
    let _ = std::fs::write(version_env_path(), lines.join("\n"));
}

fn count_file_lines() -> usize {
    std::env::var("OMB_MOCK_CODEX_COUNT_FILE")
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|c| c.lines().count())
        .unwrap_or(0)
}

fn emit_success(msg: &str) {
    event(r#"{"type":"thread.started","thread_id":"mock-thread-1"}"#);
    event(r#"{"type":"turn.started"}"#);
    event(&format!(
        "{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item_1\",\"type\":\"agent_message\",\"text\":{}}}}}",
        serde_json_escape(msg)
    ));
    event(
        r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5,"reasoning_output_tokens":0}}"#,
    );
}
fn event(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn serde_json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
