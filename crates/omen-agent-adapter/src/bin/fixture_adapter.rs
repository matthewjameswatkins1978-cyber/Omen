//! Reference adapter implementation for `omen.agent-adapter/0.1`.
//!
//! This is the small example future adapter authors copy: hello handshake,
//! capability declaration, reasoning, typed response, tool proposal,
//! error, prompt shutdown. No network. Behavior is selected by the
//! `OMB_FIXTURE_MODE` environment variable so argv stays clean.
//!
//! Human logs go to stderr only; stdout carries NDJSON frames.

use omen_agent_adapter::protocol::*;
use std::io::{BufRead, Write};

const ADAPTER_ID: &str = "omen-fixture";
const ADAPTER_VERSION: &str = "0.1.0";

fn main() {
    let mode = std::env::var("OMB_FIXTURE_MODE").unwrap_or_else(|_| "explanation".into());
    eprintln!("omen-fixture-adapter ready (mode={mode})");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut next_id: u64 = 100;
    let mut tool_roundtrip_pending: Option<String> = None;

    // Handshake: first frame must be omen.hello.
    let first = lines
        .next()
        .unwrap_or(Ok(String::new()))
        .unwrap_or_default();
    match decode_omen_frame(&first) {
        Ok(OmenFrame::Hello { id, .. }) => {
            let features = match mode.as_str() {
                "tool-roundtrip" => vec!["tools".to_string()],
                _ => vec![],
            };
            emit(&AdapterFrame::Hello {
                id,
                payload: AdapterHello {
                    protocol: format!("{ADAPTER_PROTOCOL_SCHEMA}/{ADAPTER_PROTOCOL_VERSION}"),
                    adapter_id: ADAPTER_ID.into(),
                    adapter_version: ADAPTER_VERSION.into(),
                    protocol_version: ADAPTER_PROTOCOL_VERSION.into(),
                    capabilities: vec![
                        "reasoning".into(),
                        "structured-response".into(),
                        "proposal".into(),
                        "tool-proposal".into(),
                        "deterministic".into(),
                    ],
                    features,
                    contract_versions: vec!["0.8".into()],
                    credential_labels: vec![],
                    extra: Default::default(),
                },
            });
        }
        _ => {
            emit_error(0, None, "incompatible", "expected omen.hello first");
            std::process::exit(2);
        }
    }

    for line in lines {
        let line = line.unwrap_or_default();
        if line.trim().is_empty() {
            continue;
        }
        let frame = match decode_omen_frame(&line) {
            Ok(f) => f,
            Err(e) => {
                emit_error(next_id, None, "malformed", &format!("bad omen frame: {e}"));
                next_id += 1;
                continue;
            }
        };
        match frame {
            OmenFrame::Reason { payload, .. } => {
                next_id = handle_reason(&mode, &payload, next_id, &mut tool_roundtrip_pending);
            }
            OmenFrame::ToolResult { payload, .. } => {
                if tool_roundtrip_pending.as_deref() == Some(payload.request_id.as_str()) {
                    tool_roundtrip_pending = None;
                    let digest = sha256_short(&payload.result.to_string());
                    emit(&AdapterFrame::Response {
                        id: next_id,
                        payload: AdapterResponse {
                            request_id: payload.request_id,
                            kind: "explanation".into(),
                            message: format!(
                                "fixture saw tool result (digest {digest}) and answers: ok={}",
                                payload.ok
                            ),
                            proposed_actions: vec![],
                            references: vec![],
                            uncertainty: None,
                            extra: Default::default(),
                        },
                    });
                    next_id += 1;
                }
            }
            OmenFrame::Cancel { .. } => {
                // Acknowledged termination: exit promptly, no phantom output.
                std::process::exit(0);
            }
            OmenFrame::Shutdown { .. } => {
                std::process::exit(0);
            }
            OmenFrame::Hello { .. } => {
                emit_error(next_id, None, "malformed", "duplicate hello");
                next_id += 1;
            }
        }
    }
}

fn handle_reason(
    mode: &str,
    payload: &ReasonRequest,
    mut next_id: u64,
    pending: &mut Option<String>,
) -> u64 {
    let rid = payload.request_id.clone();
    match mode {
        "tool-roundtrip" => {
            *pending = Some(rid.clone());
            emit(&AdapterFrame::ToolRequest {
                id: next_id,
                payload: AdapterToolRequest {
                    request_id: rid,
                    call_id: "call-1".into(),
                    tool: "omen.describe".into(),
                    input: serde_json::json!({"target": "fixture"}),
                },
            });
            next_id += 1;
        }
        "propose" => {
            let sentinel =
                std::env::var("OMB_FIXTURE_SENTINEL").unwrap_or_else(|_| "SENTINEL".into());
            emit(&AdapterFrame::Response {
                id: next_id,
                payload: AdapterResponse {
                    request_id: rid,
                    kind: "proposal".into(),
                    message: "fixture proposes touching the sentinel".into(),
                    proposed_actions: vec![serde_json::json!({
                        "action_type": "execute_command",
                        "argv": ["write-sentinel", sentinel],
                        "cwd": null,
                    })],
                    references: vec![],
                    uncertainty: None,
                    extra: Default::default(),
                },
            });
            next_id += 1;
        }
        "malformed" => {
            println!("THIS IS NOT JSON {{{{");
        }
        "oversize" => {
            let big = "x".repeat(MAX_FRAME_BYTES + 16);
            println!("{big}");
        }
        "stall" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        "exit-mid" => {
            std::process::exit(3);
        }
        "flood" => {
            for _ in 0..5000 {
                emit_error(next_id, Some(rid.clone()), "internal", "flood diagnostic");
                next_id += 1;
            }
            emit(&AdapterFrame::Response {
                id: next_id,
                payload: AdapterResponse {
                    request_id: rid,
                    kind: "explanation".into(),
                    message: "fixture survived its own flood".into(),
                    proposed_actions: vec![],
                    references: vec![],
                    uncertainty: None,
                    extra: Default::default(),
                },
            });
            next_id += 1;
        }
        "unknown-kind" => {
            emit(&AdapterFrame::Response {
                id: next_id,
                payload: AdapterResponse {
                    request_id: rid,
                    kind: "teleport".into(),
                    message: "unknown kinds must be rejected by Omen".into(),
                    proposed_actions: vec![],
                    references: vec![],
                    uncertainty: None,
                    extra: Default::default(),
                },
            });
            next_id += 1;
        }
        "invalid-utf8" => {
            use std::io::Write as _;
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(b"\xff\xfe not utf-8 \x80\n");
            let _ = out.flush();
        }
        "unread-exit" => {
            // Exit with stdin unread and no response: Omen must see EOF, not hang.
            std::process::exit(0);
        }
        "auth" => {
            emit_error(
                next_id,
                Some(rid),
                "auth_required",
                "fixture has no credentials",
            );
            next_id += 1;
        }
        _ => {
            if mode == "counting"
                && let Ok(count_file) = std::env::var("OMB_FIXTURE_COUNT_FILE")
                && let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&count_file)
            {
                use std::io::Write as _;
                let _ = writeln!(f, "reason");
            }
            emit(&AdapterFrame::Response {
                id: next_id,
                payload: AdapterResponse {
                    request_id: rid,
                    kind: "explanation".into(),
                    message: format!("fixture explanation for: {}", truncate(&payload.prompt)),
                    proposed_actions: vec![],
                    references: vec![],
                    uncertainty: None,
                    extra: Default::default(),
                },
            });
            next_id += 1;
        }
    }
    next_id
}

fn emit(frame: &AdapterFrame) {
    match encode_frame(frame) {
        Ok(line) => {
            print!("{line}");
            let _ = std::io::stdout().flush();
        }
        Err(e) => {
            eprintln!("fixture encode error: {e}");
            std::process::exit(2);
        }
    }
}

fn emit_error(id: u64, request_id: Option<String>, code: &str, message: &str) {
    emit(&AdapterFrame::Error {
        id,
        payload: AdapterErrorFrame {
            request_id,
            code: code.into(),
            message: message.into(),
            detail: None,
        },
    });
}

fn truncate(s: &str) -> String {
    const MAX: usize = 200;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}...", &s[..MAX])
    }
}

fn sha256_short(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(s.as_bytes())[..8])
}
