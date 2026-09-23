//! Codex route translation tests behind the deterministic mock.
//!
//! No credentials, no network, no credits. Process-global mock selection
//! is serialized with a mutex (same pattern as the G1 env guard).

use omen_agent::AgentContext;
use omen_agent::codex::*;
use omen_agent::provider::*;
use omen_agent_adapter::{EnvPolicy, codex_env_policy};
use omen_core::InteractiveSessionId;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

static MOCK_LOCK: Mutex<()> = Mutex::new(());

struct MockGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(String, Option<String>)>,
}

impl MockGuard {
    fn set(vars: &[(&str, &str)]) -> Self {
        let lock = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut previous = Vec::new();
        for (k, v) in vars {
            previous.push((k.to_string(), std::env::var(k).ok()));
            unsafe { std::env::set_var(k, v) };
        }
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for MockGuard {
    fn drop(&mut self) {
        unsafe {
            for (k, prev) in &self.previous {
                match prev {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

fn mock_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omen-mock-codex"))
}

fn parent_env(count_file: Option<&std::path::Path>) -> Vec<(String, String)> {
    let mut env = vec![
        ("SystemRoot".into(), "C:\\Windows".into()),
        ("SystemDrive".into(), "C:".into()),
        ("USERPROFILE".into(), "C:\\Users\\test".into()),
        (
            "TEMP".into(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ),
        (
            "TMP".into(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ),
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        // Synthetic secrets: the driver must never forward these.
        (
            "OPENAI_API_KEY".into(),
            "sk-codex-SYNTHETIC-NEVER-LEAK".into(),
        ),
        (
            "OMEN_SYNTHETIC_SECRET".into(),
            "synthetic-codex-secret".into(),
        ),
    ];
    // The mock's control knobs travel via the real process env (set by the
    // guard) and are relayed by a test-only policy extension below.
    if let Ok(v) = std::env::var("OMB_MOCK_CODEX_TRANSCRIPT") {
        env.push(("OMB_MOCK_CODEX_TRANSCRIPT".into(), v));
    }
    if let Ok(v) = std::env::var("OMB_MOCK_CODEX_MESSAGE") {
        env.push(("OMB_MOCK_CODEX_MESSAGE".into(), v));
    }
    if let Ok(v) = std::env::var("OMB_MOCK_CODEX_MESSAGE_2") {
        env.push(("OMB_MOCK_CODEX_MESSAGE_2".into(), v));
    }
    if let Some(path) = count_file {
        env.push((
            "OMB_MOCK_CODEX_COUNT_FILE".into(),
            path.to_string_lossy().into_owned(),
        ));
    }
    env
}

fn test_policy() -> EnvPolicy {
    // Test-only extension: relay mock control knobs; everything else is the
    // production Codex policy (which drops OPENAI_API_KEY).
    let mut policy = codex_env_policy();
    policy.allow_vars.push("OMB_MOCK_CODEX_TRANSCRIPT".into());
    policy.allow_vars.push("OMB_MOCK_CODEX_MESSAGE".into());
    policy.allow_vars.push("OMB_MOCK_CODEX_MESSAGE_2".into());
    policy.allow_vars.push("OMB_MOCK_CODEX_COUNT_FILE".into());
    policy
}

fn adapter_with(timeout: Duration, count_file: Option<&std::path::Path>) -> CodexAdapter {
    let mut config = CodexRouteConfig::new(mock_exe(), parent_env(count_file));
    config.timeout = timeout;
    config.env_policy = test_policy();
    CodexAdapter::new(config)
}

fn request(prompt: &str) -> AgentRequest {
    let cwd = std::env::temp_dir();
    AgentRequest {
        prompt: prompt.into(),
        context: AgentContext::new(
            "ws-codex",
            cwd.clone(),
            InteractiveSessionId::new("s").unwrap(),
            cwd,
        ),
        conversation: vec![],
    }
}

fn count_spawns(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|c| c.lines().count())
        .unwrap_or(0)
}

fn unique_count_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "g2-codex-count-{tag}-{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

// ---------- translation ----------

#[tokio::test]
async fn success_transcript_becomes_explanation_with_one_spawn() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "success")]);
    let count = unique_count_file("success");
    let adapter = adapter_with(Duration::from_secs(20), Some(&count));
    let resp = adapter.respond(request("hi")).await.unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Explanation);
    assert!(resp.message.contains("mock codex explanation"));
    assert_eq!(
        count_spawns(&count),
        1,
        "no silent retry: one spawn per request"
    );
    let _ = std::fs::remove_file(&count);
}

#[tokio::test]
async fn tool_round_trip_executes_then_answers() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "tool-roundtrip")]);
    let count = unique_count_file("tool");
    let adapter = adapter_with(Duration::from_secs(30), Some(&count));
    let resp = adapter
        .respond(request("what is the workspace?"))
        .await
        .unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Explanation);
    assert!(resp.message.contains("incorporating tool result"));
    assert_eq!(
        count_spawns(&count),
        2,
        "exactly one tool round trip: two spawns"
    );
    let _ = std::fs::remove_file(&count);
}

#[tokio::test]
async fn second_tool_ask_is_refused_not_looped() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "tool-twice")]);
    let count = unique_count_file("twice");
    let adapter = adapter_with(Duration::from_secs(30), Some(&count));
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(matches!(err, AgentError::Rejected(_)), "{err:?}");
    assert_eq!(count_spawns(&count), 2, "bounded: no third spawn");
    let _ = std::fs::remove_file(&count);
}

#[tokio::test]
async fn proposal_argv_becomes_unexecuted_proposal() {
    let _guard = MockGuard::set(&[
        ("OMB_MOCK_CODEX_TRANSCRIPT", "success"),
        (
            "OMB_MOCK_CODEX_MESSAGE",
            r#"{"message":"consider this","proposal_argv":["write-sentinel","X"]}"#,
        ),
    ]);
    let adapter = adapter_with(Duration::from_secs(20), None);
    let resp = adapter
        .respond(request("consider the sentinel"))
        .await
        .unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Proposal);
    assert_eq!(resp.proposed_actions.len(), 1);
    match &resp.proposed_actions[0] {
        ProposedAction::ExecuteCommand { argv, .. } => {
            assert_eq!(argv, &vec!["write-sentinel".to_string(), "X".to_string()]);
        }
        other => panic!("expected ExecuteCommand data, got {other:?}"),
    }
}

#[tokio::test]
async fn non_allowlisted_tool_never_executes() {
    let _guard = MockGuard::set(&[
        ("OMB_MOCK_CODEX_TRANSCRIPT", "tool-roundtrip"),
        (
            "OMB_MOCK_CODEX_MESSAGE",
            r#"{"message":"evil","tool_requests":[{"tool":"omen.evil","operation":"x","input":{}}]}"#,
        ),
    ]);
    let count = unique_count_file("evil");
    let adapter = adapter_with(Duration::from_secs(30), Some(&count));
    // The evil tool is refused inside the round trip; the final answer still
    // arrives. Nothing outside the allowlist ever executes.
    let resp = adapter.respond(request("hi")).await.unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Explanation);
    let _ = std::fs::remove_file(&count);
}

// ---------- failure translation ----------

#[tokio::test]
async fn malformed_stream_is_rejected() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "malformed")]);
    let adapter = adapter_with(Duration::from_secs(20), None);
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(matches!(err, AgentError::Rejected(_)), "{err:?}");
}

#[tokio::test]
async fn auth_failure_maps_to_authentication_required() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "auth-fail")]);
    let adapter = adapter_with(Duration::from_secs(20), None);
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(
        matches!(err, AgentError::AuthenticationRequired { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn rate_limit_maps_cleanly() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "rate-limited")]);
    let adapter = adapter_with(Duration::from_secs(20), None);
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(matches!(err, AgentError::RateLimited { .. }), "{err:?}");
}

#[tokio::test]
async fn stall_hits_bounded_timeout() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "stall")]);
    let adapter = adapter_with(Duration::from_secs(3), None);
    let started = std::time::Instant::now();
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(matches!(err, AgentError::Timeout(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[tokio::test]
async fn crash_is_explicit_with_no_synthetic_response() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "crash")]);
    let adapter = adapter_with(Duration::from_secs(20), None);
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(matches!(err, AgentError::Provider(_)), "{err:?}");
}

#[tokio::test]
async fn empty_success_is_rejected_not_synthesized() {
    let _guard = MockGuard::set(&[("OMB_MOCK_CODEX_TRANSCRIPT", "empty")]);
    let adapter = adapter_with(Duration::from_secs(20), None);
    let err = adapter.respond(request("hi")).await.unwrap_err();
    assert!(matches!(err, AgentError::Rejected(_)), "{err:?}");
}

// ---------- prompt economy + parsing ----------

#[test]
fn bootstrap_prompt_stays_within_budget() {
    let long = "x".repeat(100_000);
    let prompt = build_codex_prompt(&request(&long), 1, None);
    assert!(
        prompt.len() <= CODEX_BOOTSTRAP_BUDGET_BYTES,
        "bootstrap {} bytes exceeds budget",
        prompt.len()
    );
    assert!(prompt.contains("read-only"));
    assert!(prompt.contains("omen.workspace_status"));
    assert!(!prompt.contains("sk-"));
}

#[test]
fn final_parsing_strict_then_bounded_fallback() {
    let clean = parse_codex_final(r#"{"message":"hi"}"#).unwrap();
    assert_eq!(clean.message, "hi");
    let wrapped =
        parse_codex_final("Some chatter {\"message\":\"there\",\"proposal_argv\":null} trailing")
            .unwrap();
    assert_eq!(wrapped.message, "there");
    assert!(parse_codex_final("no json at all").is_err());
    assert!(parse_codex_final(r#"{"message":123}"#).is_err());
    assert!(parse_codex_final(&format!(r#"{{"message":"{}"}}"#, "y".repeat(9000))).is_err());
}

// ---------- secrets + identity ----------

#[test]
fn driver_default_policy_drops_unrelated_secrets() {
    let config = CodexRouteConfig::new(mock_exe(), parent_env(None));
    assert_eq!(config.env_policy, codex_env_policy());
    let env = config
        .env_policy
        .build_env(&config.parent_env, &["codex-auth".into()]);
    EnvPolicy::assert_no_secret_leak(
        &env,
        &["sk-codex-SYNTHETIC-NEVER-LEAK", "synthetic-codex-secret"],
    )
    .unwrap();
    assert!(env.iter().any(|(k, _)| k == "USERPROFILE"));
    assert!(!env.iter().any(|(k, _)| k == "OPENAI_API_KEY"));
}

#[test]
fn descriptor_names_codex_truthfully() {
    let desc = codex_descriptor(true);
    assert_eq!(desc.id, "codex");
    assert_eq!(desc.model, None, "server-assigned model stays UNKNOWN");
    assert!(desc.credential_source.unwrap().contains("codex-auth"));
    assert!(!desc.capabilities.iter().any(|c| c.contains("codex")));
}
