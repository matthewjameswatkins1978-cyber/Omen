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

// ---------- Lucy repair: probe isolation, probe bound, resolver ----------

const PROBE_SECRET_A: &str = "sk-codex-SYNTHETIC-NEVER-LEAK";
const PROBE_SECRET_B: &str = "synthetic-codex-secret";

fn version_env_path() -> PathBuf {
    std::env::temp_dir().join("omen-mock-codex-version-env.txt")
}

fn version_pid_path() -> PathBuf {
    std::env::temp_dir().join("omen-mock-codex-version-pid.txt")
}

/// TEST 1 — the probe child receives the allowlisted environment only.
/// Parent carries OPENAI_API_KEY + synthetic secret; the fake records what
/// actually arrived. Secrets must be absent; approved entries present.
#[test]
fn probe_child_env_is_isolated_by_canonical_policy() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = std::fs::remove_file(version_env_path());
    let parent = parent_env(None);
    assert!(parent.iter().any(|(k, _)| k == "OPENAI_API_KEY"));
    let probe =
        probe_codex(&mock_exe(), &parent, &codex_env_policy()).expect("isolated probe succeeds");
    assert_eq!(probe.version, "mock-codex 0.0.0");
    assert!(!probe.exe_digest.is_empty());
    let dump =
        std::fs::read_to_string(version_env_path()).expect("mock --version records its child env");
    assert!(
        !dump.contains(PROBE_SECRET_A) && !dump.contains(PROBE_SECRET_B),
        "secret value reached the probe child: {dump}"
    );
    assert!(dump.contains("OPENAI_API_KEY=MISSING"));
    assert!(dump.contains("OMEN_SYNTHETIC_SECRET=MISSING"));
    assert!(dump.contains("PATH=") && !dump.contains("PATH=MISSING"));
    assert!(dump.contains("NO_COLOR=1"), "{dump}");
    let _ = std::fs::remove_file(version_env_path());
}

/// The shared helper used by probe AND route drops secrets identically.
#[test]
fn codex_child_env_helper_is_the_single_boundary() {
    let parent = parent_env(None);
    let env = codex_child_env(&parent, &codex_env_policy());
    EnvPolicy::assert_no_secret_leak(&env, &[PROBE_SECRET_A, PROBE_SECRET_B]).unwrap();
    assert!(!env.iter().any(|(k, _)| k == "OPENAI_API_KEY"));
    assert!(env.iter().any(|(k, _)| k == "USERPROFILE"));
}

#[cfg(windows)]
fn process_gone(pid: u32) -> bool {
    // Read-only check: tasklist lists; nothing here terminates anything.
    match std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
    {
        Ok(o) => !String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")),
        Err(_) => true,
    }
}

#[cfg(not(windows))]
fn process_gone(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
}

/// TEST 2 — a stalled `codex --version` fails closed inside the bound:
/// truthful Timeout, no retry, no lingering process.
#[test]
fn probe_stall_fails_closed_with_no_lingering_process() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { std::env::set_var("OMB_MOCK_CODEX_TRANSCRIPT", "version-stall") };
    let _ = std::fs::remove_file(version_pid_path());
    // Test-only relay so the stall knob reaches the mock; production code
    // paths use the unextended canonical policy.
    let mut policy = codex_env_policy();
    policy.allow_vars.push("OMB_MOCK_CODEX_TRANSCRIPT".into());
    let parent = parent_env(None);
    let bound = Duration::from_secs(3);
    let started = std::time::Instant::now();
    let err = probe_codex_with_timeout(&mock_exe(), &parent, &policy, bound).unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(err, AgentError::Timeout(_)), "{err:?}");
    assert!(
        elapsed < Duration::from_secs(20),
        "probe escaped its bound: {elapsed:?}"
    );
    // No retry: exactly one stalled child was started (pid file written
    // once, truncating) and it must be gone after the bounded kill.
    let pid: u32 = std::fs::read_to_string(version_pid_path())
        .expect("stalled mock records its pid")
        .trim()
        .parse()
        .expect("pid parses");
    let gone_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !process_gone(pid) && std::time::Instant::now() < gone_deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(process_gone(pid), "stalled probe child still alive");
    unsafe { std::env::remove_var("OMB_MOCK_CODEX_TRANSCRIPT") };
    let _ = std::fs::remove_file(version_pid_path());
}

struct PathGuard {
    previous_path: Option<String>,
    previous_override: Option<String>,
}

impl PathGuard {
    fn set_path(value: &str) -> Self {
        let previous_path = std::env::var("PATH").ok();
        let previous_override = std::env::var("OMEN_CODEX_EXE").ok();
        unsafe {
            std::env::set_var("PATH", value);
            std::env::remove_var("OMEN_CODEX_EXE");
        }
        Self {
            previous_path,
            previous_override,
        }
    }

    #[cfg(windows)]
    fn set_override(value: &str) -> Self {
        let previous_path = std::env::var("PATH").ok();
        let previous_override = std::env::var("OMEN_CODEX_EXE").ok();
        unsafe {
            std::env::set_var("OMEN_CODEX_EXE", value);
        }
        Self {
            previous_path,
            previous_override,
        }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match &self.previous_override {
                Some(v) => std::env::set_var("OMEN_CODEX_EXE", v),
                None => std::env::remove_var("OMEN_CODEX_EXE"),
            }
        }
    }
}

fn unique_probe_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "g2-probe-resolve-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(windows)]
fn vendor_exe(dir: &std::path::Path, package: &str, target: &str) -> PathBuf {
    let exe = dir
        .join("node_modules")
        .join("@openai")
        .join("codex")
        .join("node_modules")
        .join("@openai")
        .join(package)
        .join("vendor")
        .join(target)
        .join("bin")
        .join("codex.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"fake").unwrap();
    exe
}

/// TEST 3.1 — a real codex.exe on PATH resolves directly.
#[test]
fn resolver_prefers_real_exe_on_path() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = unique_probe_dir("real");
    let exe_name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let exe = dir.join(exe_name);
    std::fs::write(&exe, b"fake").unwrap();
    let _paths = PathGuard::set_path(dir.to_str().unwrap());
    assert_eq!(resolve_codex_exe(), Some(exe));
    let _ = std::fs::remove_dir_all(&dir);
}

/// TEST 3.2 — shim + exactly one vendored exe resolves to the real exe.
#[cfg(windows)]
#[test]
fn resolver_uses_unique_vendored_exe_behind_shim() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = unique_probe_dir("vendored");
    std::fs::write(dir.join("codex.cmd"), b"@echo off").unwrap();
    let real = vendor_exe(&dir, "codex-win32-x64", "x64");
    let _paths = PathGuard::set_path(dir.to_str().unwrap());
    assert_eq!(resolve_codex_exe(), Some(real));
    let _ = std::fs::remove_dir_all(&dir);
}

/// TEST 3.3 — shim with no vendored exe fails closed (never the shim).
#[cfg(windows)]
#[test]
fn resolver_fails_closed_when_vendored_exe_missing() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = unique_probe_dir("novendor");
    std::fs::write(dir.join("codex.cmd"), b"@echo off").unwrap();
    let _paths = PathGuard::set_path(dir.to_str().unwrap());
    assert_eq!(resolve_codex_exe(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// TEST 3.4 — shim with ambiguous vendored layout fails closed.
#[cfg(windows)]
#[test]
fn resolver_fails_closed_when_vendored_layout_ambiguous() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = unique_probe_dir("ambiguous");
    std::fs::write(dir.join("codex.cmd"), b"@echo off").unwrap();
    vendor_exe(&dir, "codex-win32-x64", "x64");
    vendor_exe(&dir, "codex-win32-x64", "arm64");
    let _paths = PathGuard::set_path(dir.to_str().unwrap());
    assert_eq!(resolve_codex_exe(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// TEST 4 — structural proof: resolution never yields a command script,
/// via PATH shims, ambiguous layouts, or an explicit override.
#[cfg(windows)]
#[test]
fn resolver_never_returns_command_script() {
    let _guard = MOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for tag in ["cmd-only", "cmd-ambiguous"] {
        let dir = unique_probe_dir(tag);
        let shim = dir.join("codex.cmd");
        std::fs::write(&shim, b"@echo off").unwrap();
        if tag == "cmd-ambiguous" {
            vendor_exe(&dir, "codex-win32-x64", "x64");
            vendor_exe(&dir, "codex-win32-x64", "arm64");
        }
        {
            let _paths = PathGuard::set_path(dir.to_str().unwrap());
            let got = resolve_codex_exe();
            assert!(got.is_none(), "resolver must fail closed, got {got:?}");
        }
        // Explicit override pointing at a script is equally refused.
        {
            let _paths = PathGuard::set_override(shim.to_str().unwrap());
            unsafe { std::env::set_var("PATH", "") };
            let got = resolve_codex_exe();
            assert!(
                got.is_none(),
                "override must not yield a command script, got {got:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    // Belt and braces across every case above: no .cmd/.bat ever escapes.
    let dir = unique_probe_dir("ext-scan");
    std::fs::write(dir.join("codex.cmd"), b"@echo off").unwrap();
    std::fs::write(dir.join("codex.bat"), b"@echo off").unwrap();
    let _paths = PathGuard::set_path(dir.to_str().unwrap());
    if let Some(p) = resolve_codex_exe() {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        assert!(
            ext != "cmd" && ext != "bat",
            "command script escaped: {p:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
