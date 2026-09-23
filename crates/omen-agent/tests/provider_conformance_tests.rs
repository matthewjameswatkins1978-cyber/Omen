//! G1 provider conformance battery.
//!
//! Full 21-scenario run against the deterministic canary, supported subsets
//! for DiagnosticAgentProvider and the mocked OpenAI Responses transport,
//! plus bounds, registry startup, capability vocabulary, secret hygiene, and
//! the provider-ontology tripwire. Normal CI never touches the network and
//! never spends API credits (synthetic keys only).

use omen_agent::{
    AgentContext, AgentError, AgentProvider, AgentRequest, AgentResponse, AgentResponseKind,
    ConformanceMode, ConformanceProvider, ConformanceScenario, HttpPost, HttpResponseSnapshot,
    OpenAiResponsesProvider, ProviderCapability, ProviderRegistry, TimeoutProvider, check_scenario,
    is_canonical_capability, openai_luna_config,
};
use omen_core::InteractiveSessionId;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Serializes tests that mutate OPENAI_API_KEY in the process environment.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct EnvGuard {
    previous: Option<String>,
}

impl EnvGuard {
    fn set(value: Option<&str>) -> Self {
        let previous = std::env::var("OPENAI_API_KEY").ok();
        unsafe {
            match value {
                Some(v) => std::env::set_var("OPENAI_API_KEY", v),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
        }
        Self { previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var("OPENAI_API_KEY", v),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
        }
    }
}

fn with_openai_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = EnvGuard::set(value);
    f()
}

fn base_request(prompt: &str, cwd: &Path) -> AgentRequest {
    AgentRequest {
        prompt: prompt.into(),
        context: AgentContext::new(
            "ws-g1",
            cwd.to_path_buf(),
            InteractiveSessionId::new("sess-g1").unwrap(),
            cwd.to_path_buf(),
        ),
        conversation: Vec::new(),
    }
}

/// Counting mocked transport: asserts single invocation (no silent retry).
struct CountingMock {
    status: u16,
    body: String,
    err: Option<String>,
    calls: AtomicUsize,
}

impl CountingMock {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            body: body.into(),
            err: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.into(),
            err: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn transport_error(msg: &str) -> Self {
        Self {
            status: 0,
            body: String::new(),
            err: Some(msg.into()),
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HttpPost for CountingMock {
    fn post_json(
        &self,
        _url: &str,
        _headers: &[(String, String)],
        _body: &str,
    ) -> Result<HttpResponseSnapshot, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.err {
            Some(e) => Err(e.clone()),
            None => Ok(HttpResponseSnapshot {
                status: self.status,
                retry_after_secs: None,
                body: self.body.clone(),
            }),
        }
    }
}

fn mock_luna(
    mock: CountingMock,
    key: Option<&str>,
) -> (OpenAiResponsesProvider, Arc<CountingMock>) {
    let mock = Arc::new(mock);
    let provider = OpenAiResponsesProvider::with_http(
        openai_luna_config(),
        key.map(str::to_string),
        mock.clone(),
        "openai-luna",
    );
    (provider, mock)
}

fn completed_envelope(kind: &str, message: &str, actions: serde_json::Value) -> String {
    let inner = serde_json::json!({
        "kind": kind,
        "message": message,
        "proposed_actions": actions,
        "references": [],
        "uncertainty": null,
    });
    serde_json::json!({
        "status": "completed",
        "output": [{
            "type": "message",
            "content": [{
                "type": "output_text",
                "text": inner.to_string(),
            }],
        }],
    })
    .to_string()
}

fn completed_envelope_raw(kind: &str, message: &str, actions_json: &str) -> String {
    let actions: serde_json::Value = serde_json::from_str(actions_json).unwrap();
    completed_envelope(kind, message, actions)
}

fn error_envelope(message: &str) -> String {
    serde_json::json!({"error": {"message": message}}).to_string()
}

// ---------------------------------------------------------------------------
// Full canary battery: all 21 scenarios
// ---------------------------------------------------------------------------

fn scenario_mode(scenario: ConformanceScenario) -> ConformanceMode {
    match scenario {
        ConformanceScenario::SuccessExplanation => ConformanceMode::SuccessExplanation,
        ConformanceScenario::SuccessProposal => ConformanceMode::SuccessProposal,
        ConformanceScenario::SuccessActionRequest => ConformanceMode::SuccessActionRequest,
        ConformanceScenario::SuccessQuestion => ConformanceMode::SuccessQuestion,
        ConformanceScenario::SuccessRefusal => ConformanceMode::SuccessRefusal,
        ConformanceScenario::AuthMissing => ConformanceMode::AuthMissing,
        ConformanceScenario::AuthRejected => ConformanceMode::AuthRejected,
        ConformanceScenario::ProviderUnavailable => ConformanceMode::ProviderUnavailable,
        ConformanceScenario::ModelUnavailable => ConformanceMode::ModelUnavailable,
        ConformanceScenario::RateLimited => ConformanceMode::RateLimited,
        ConformanceScenario::Timeout => ConformanceMode::TimeoutNow,
        ConformanceScenario::MalformedResponse => ConformanceMode::Malformed,
        ConformanceScenario::UnsupportedCapability => ConformanceMode::UnsupportedCapability,
        ConformanceScenario::TransportFailure => ConformanceMode::TransportFailure,
        ConformanceScenario::DisconnectBeforeResponse => ConformanceMode::Disconnect,
        ConformanceScenario::ResponseTooLarge => ConformanceMode::ResponseTooLarge,
        ConformanceScenario::TooManyActions => ConformanceMode::TooManyActions,
        ConformanceScenario::MalformedAction => ConformanceMode::MalformedAction,
        ConformanceScenario::UnknownResponseKind => ConformanceMode::UnknownKind,
        ConformanceScenario::IncompleteProviderResponse => ConformanceMode::Incomplete,
        ConformanceScenario::ProviderRecoversAfterFailure => {
            ConformanceMode::Script(VecDeque::from([
                ConformanceMode::TransportFailure,
                ConformanceMode::SuccessExplanation,
            ]))
        }
    }
}

#[test]
fn canary_passes_full_conformance_battery() {
    let dir = tempdir().unwrap();
    let mut failures = Vec::new();
    for scenario in ConformanceScenario::all() {
        let canary = ConformanceProvider::new(scenario_mode(scenario));
        let req = base_request("canary probe", dir.path());
        match check_scenario(scenario, &canary, &req) {
            Ok(detail) => {
                let expected_calls =
                    if scenario == ConformanceScenario::ProviderRecoversAfterFailure {
                        2
                    } else {
                        1
                    };
                assert_eq!(
                    canary.call_count(),
                    expected_calls,
                    "{}: expected {expected_calls} invocation(s)",
                    scenario.name()
                );
                assert!(
                    detail.len() > 4,
                    "{}: probe detail must say something",
                    scenario.name()
                );
            }
            Err(detail) => failures.push(format!("{}: {detail}", scenario.name())),
        }
    }
    assert!(
        failures.is_empty(),
        "canary battery failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn canary_captures_request_and_counts_exactly() {
    let dir = tempdir().unwrap();
    let canary = ConformanceProvider::new(ConformanceMode::SuccessExplanation);
    let req = base_request("count me exactly once", dir.path());
    let resp = omen_agent::context::block_on_async(canary.respond(req));
    assert!(resp.is_ok());
    assert_eq!(canary.call_count(), 1);
    let seen = canary.last_request().expect("request captured");
    assert_eq!(seen.prompt, "count me exactly once");
    assert_eq!(seen.context.cwd, dir.path());
    // No credential material can flow through the canary (it holds none).
    let flat = serde_json::to_string(&seen.context).unwrap();
    assert!(!flat.to_lowercase().contains("bearer"));
}

#[test]
fn canary_script_exhaustion_is_explicit_not_replay() {
    let dir = tempdir().unwrap();
    let canary = ConformanceProvider::new(ConformanceMode::Script(VecDeque::from([
        ConformanceMode::SuccessExplanation,
    ])));
    let req = base_request("script", dir.path());
    let first = omen_agent::context::block_on_async(canary.respond(req.clone()));
    assert!(first.is_ok());
    let second = omen_agent::context::block_on_async(canary.respond(req));
    match second {
        Err(AgentError::Provider(msg)) => assert!(msg.contains("exhausted")),
        other => panic!("exhaustion must be explicit, got {other:?}"),
    }
    assert_eq!(canary.call_count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn real_timeout_ceiling_fires_on_stalled_provider() {
    let dir = tempdir().unwrap();
    let canary = ConformanceProvider::new(ConformanceMode::Sleep(Duration::from_secs(30)));
    let wrapped = TimeoutProvider::new(Arc::new(canary), Duration::from_millis(100));
    let req = base_request("stall", dir.path());
    match check_scenario(ConformanceScenario::Timeout, &wrapped, &req) {
        Ok(detail) => assert!(detail.contains("timeout")),
        Err(detail) => panic!("timeout ceiling must fire: {detail}"),
    }
}

// ---------------------------------------------------------------------------
// Diagnostic subset (only capabilities the provider claims)
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_passes_claimed_success_subset() {
    let dir = tempdir().unwrap();
    let diag = omen_agent::DiagnosticAgentProvider::new();
    for (scenario, prompt) in [
        (
            ConformanceScenario::SuccessExplanation,
            "what folder am I in?",
        ),
        (
            ConformanceScenario::SuccessProposal,
            "check whether this project still builds",
        ),
        (
            ConformanceScenario::SuccessRefusal,
            "take me to nowhere-zzz-no-such-dir",
        ),
    ] {
        let req = base_request(prompt, dir.path());
        match check_scenario(scenario, &diag, &req) {
            Ok(_) => {}
            Err(detail) => panic!("diagnostic {} failed: {detail}", scenario.name()),
        }
    }
    // Ambiguous navigation -> typed Question (not a guess, not a failure).
    std::fs::create_dir_all(dir.path().join("alpha-one")).unwrap();
    std::fs::create_dir_all(dir.path().join("alpha-two")).unwrap();
    let req = base_request("take me to alpha", dir.path());
    match check_scenario(ConformanceScenario::SuccessQuestion, &diag, &req) {
        Ok(_) => {}
        Err(detail) => panic!("diagnostic SUCCESS_QUESTION failed: {detail}"),
    }
}

// ---------------------------------------------------------------------------
// Mocked Luna subset (mocked HttpPost; zero network, zero credits)
// ---------------------------------------------------------------------------

#[test]
fn luna_mock_success_and_refusal_shapes() {
    let dir = tempdir().unwrap();
    let (provider, mock) = mock_luna(
        CountingMock::ok(&completed_envelope_raw(
            "explanation",
            "mocked luna says hi",
            "[]",
        )),
        Some("sk-g1-synthetic"),
    );
    let req = base_request("hello", dir.path());
    match check_scenario(ConformanceScenario::SuccessExplanation, &provider, &req) {
        Ok(_) => {}
        Err(d) => panic!("mocked success failed: {d}"),
    }
    assert_eq!(mock.call_count(), 1);

    let (provider, _) = mock_luna(
        CountingMock::ok(&completed_envelope_raw(
            "refusal",
            "mocked luna refuses",
            "[]",
        )),
        Some("sk-g1-synthetic"),
    );
    match check_scenario(
        ConformanceScenario::SuccessRefusal,
        &provider,
        &base_request("hello", dir.path()),
    ) {
        Ok(_) => {}
        Err(d) => panic!("mocked refusal failed: {d}"),
    }
}

#[test]
fn luna_mock_failure_mapping_and_single_invocation() {
    let dir = tempdir().unwrap();
    let cases: Vec<(ConformanceScenario, CountingMock, Option<&str>)> = vec![
        (
            ConformanceScenario::AuthRejected,
            CountingMock::status(401, &error_envelope("bad key")),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::AuthMissing,
            CountingMock::ok("{}"),
            None,
        ),
        (
            ConformanceScenario::RateLimited,
            CountingMock::status(429, &error_envelope("slow down")),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::ProviderUnavailable,
            CountingMock::status(
                404,
                &error_envelope("The model `gpt-6-luna` does not exist"),
            ),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::ModelUnavailable,
            CountingMock::status(403, &error_envelope("not entitled to this model")),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::UnsupportedCapability,
            CountingMock::status(400, &error_envelope("unsupported reasoning effort")),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::TransportFailure,
            CountingMock::transport_error("connection reset by peer"),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::MalformedResponse,
            CountingMock::ok("this is not json{{{"),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::MalformedResponse,
            CountingMock::ok(r#"{"status":"completed","output":[]}"#),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::IncompleteProviderResponse,
            CountingMock::ok(r#"{"status":"failed","error":{"message":"boom"}}"#),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::ResponseTooLarge,
            CountingMock::ok(&completed_envelope(
                "explanation",
                &"M".repeat(100_000),
                serde_json::Value::Array(Vec::new()),
            )),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::UnknownResponseKind,
            CountingMock::ok(&completed_envelope_raw("telepathy", "hmm", "[]")),
            Some("sk-g1-synthetic"),
        ),
        (
            ConformanceScenario::MalformedAction,
            CountingMock::ok(&completed_envelope_raw(
                "proposal",
                "bad action",
                r#"[{"action_type":"execute_command","argv":[]}]"#,
            )),
            Some("sk-g1-synthetic"),
        ),
    ];
    // 33 actions exceeds the transport maximum of 32.
    let many: Vec<String> = (0..33)
        .map(|i| format!(r#"{{"action_type":"semantic_action","action":"show","args":["{i}"]}}"#))
        .collect();
    let mut cases = cases;
    cases.push((
        ConformanceScenario::TooManyActions,
        CountingMock::ok(&completed_envelope_raw(
            "proposal",
            "too many",
            &format!("[{}]", many.join(",")),
        )),
        Some("sk-g1-synthetic"),
    ));

    for (scenario, mock, key) in cases {
        let (provider, counting) = mock_luna(mock, key);
        let req = base_request("probe", dir.path());
        // Missing credential must fail before any transport call (zero calls
        // is correct); every other failure happens after exactly one call.
        let expected_calls = if key.is_none() { 0 } else { 1 };
        match check_scenario(scenario, &provider, &req) {
            Ok(detail) => {
                assert_eq!(
                    counting.call_count(),
                    expected_calls,
                    "{}: one user request must be {expected_calls} provider call(s) ({detail})",
                    scenario.name()
                );
            }
            Err(detail) => panic!("{} failed: {detail}", scenario.name()),
        }
    }
}

#[test]
fn luna_mock_availability_is_not_admission() {
    // Reachable transport + HTTP 200 with invalid payload = rejected
    // reasoning, not successful reasoning.
    let dir = tempdir().unwrap();
    let (provider, mock) = mock_luna(
        CountingMock::ok(r#"{"status":"completed","output":"not-an-array"}"#),
        Some("sk-g1-synthetic"),
    );
    let err = omen_agent::context::block_on_async(provider.respond(base_request("hi", dir.path())))
        .expect_err("reachable-but-malformed must fail");
    assert!(
        matches!(err, AgentError::Rejected(_)),
        "availability must not become admission, got {err:?}"
    );
    assert_eq!(mock.call_count(), 1);
}

// ---------------------------------------------------------------------------
// Request/response bounds
// ---------------------------------------------------------------------------

#[test]
fn luna_request_body_stays_bounded_under_hostile_input() {
    let dir = tempdir().unwrap();
    let (provider, _) = mock_luna(CountingMock::ok("{}"), Some("sk-g1-synthetic-key"));
    let mut req = base_request(&"P".repeat(200_000), dir.path());
    req.conversation = (0..100)
        .map(|i| omen_agent::AgentTurn {
            role: "user".into(),
            text: format!("turn {i} {}", "x".repeat(5_000)),
            timestamp: "2026-01-01T00:00:00Z".into(),
        })
        .collect();
    req.context.available_tools = (0..10_000).map(|i| format!("tool-{i}")).collect();
    let body = provider.build_request_body(&req).expect("body builds");
    let flat = body.to_string();
    assert!(
        flat.len() < 128 * 1024,
        "request body must stay bounded, got {} bytes",
        flat.len()
    );
    assert!(!flat.contains("sk-g1-synthetic-key"));
    assert!(
        flat.contains("bounded AgentContext omitted oversized payload"),
        "oversized context must be explicitly truncated"
    );
}

// ---------------------------------------------------------------------------
// Registry: startup cost, selection truth, machine output
// ---------------------------------------------------------------------------

#[test]
fn registry_construction_is_cheap_and_credential_gated() {
    with_openai_env(Some("sk-g1-synthetic"), || {
        let start = Instant::now();
        let registry = ProviderRegistry::new();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "registry construction must not probe the network"
        );
        assert!(
            registry
                .list_providers()
                .iter()
                .any(|p| p.id == "openai-luna"),
            "credential presence registers luna"
        );
        assert_eq!(registry.active_descriptor().id, "diagnostic");
    });
    with_openai_env(None, || {
        let registry = ProviderRegistry::new();
        assert!(
            !registry
                .list_providers()
                .iter()
                .any(|p| p.id == "openai-luna"),
            "luna must be absent without credential"
        );
    });
}

#[test]
fn registry_failed_selection_preserves_active() {
    let registry = ProviderRegistry::new();
    let before = registry.active_descriptor().id.clone();
    match registry.set_active_provider("no-such-provider") {
        Err(AgentError::ProviderUnavailable { .. }) => {}
        other => panic!("unknown provider must fail explicitly, got {other:?}"),
    }
    assert_eq!(registry.active_descriptor().id, before);
}

#[test]
fn registry_capabilities_use_canonical_vocabulary() {
    for key in [Some("sk-g1-synthetic"), None] {
        with_openai_env(key, || {
            let registry = ProviderRegistry::new();
            for desc in registry.list_providers() {
                for cap in &desc.capabilities {
                    assert!(
                        is_canonical_capability(cap),
                        "provider {} advertises non-canonical capability {cap:?}",
                        desc.id
                    );
                }
            }
        });
    }
    for cap in ProviderCapability::all() {
        assert_eq!(
            ProviderCapability::parse(cap.as_str()),
            Some(cap),
            "capability round-trip"
        );
    }
    assert_eq!(ProviderCapability::parse("luna-fast"), None);
    assert_eq!(ProviderCapability::parse("openai-json"), None);
    assert_eq!(ProviderCapability::parse(""), None);
}

#[test]
fn registry_machine_output_never_carries_secrets() {
    with_openai_env(Some("sk-g1-synthetic-secret"), || {
        let registry = ProviderRegistry::new();
        let status = registry.status_text();
        assert!(status.contains("diagnostic"));
        assert!(status.contains("Capabilities:"));
        assert!(!status.contains("sk-g1-synthetic-secret"));
        assert!(!status.to_lowercase().contains("bearer"));
        for desc in registry.list_providers() {
            let json = serde_json::to_string(&desc).unwrap();
            // The secret VALUE must never appear; the source label naming the
            // environment variable is allowed and expected.
            assert!(!json.contains("sk-g1-synthetic-secret"));
            assert!(json.contains("environment:OPENAI_API_KEY") || desc.id == "diagnostic");
            let lower = json.to_lowercase();
            assert!(!lower.contains("secret\""));
            assert!(!lower.contains("bearer"));
        }
    });
}

// ---------------------------------------------------------------------------
// Provider ontology tripwire: vendor concepts must not leak past the adapter
// ---------------------------------------------------------------------------

#[test]
fn no_vendor_ontology_outside_adapter_zone() {
    // Brand/model/vendor-protocol literals may appear in the omen-agent
    // adapter zone (transport, registry presets, tests) and docs. They must
    // NOT control authority, execution, grammar, machine truth, history,
    // completion, or policy code elsewhere.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("..").join("..");
    let zones = [
        "crates/omen-interactive/src",
        "crates/omen-engine/src",
        "crates/omen-core/src",
        "crates/omen-cli/src",
        "crates/omen-daemon/src",
    ];
    let banned = [
        "openai-luna",
        "gpt-6-luna",
        "anthropic",
        "claude",
        "gemini",
        "nebius",
        "finish_reason",
        "stop_reason",
    ];
    let mut hits = Vec::new();
    for zone in zones {
        let dir = root.join(zone);
        if !dir.is_dir() {
            continue;
        }
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let entries = std::fs::read_dir(&d).unwrap();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    for (i, line) in text.lines().enumerate() {
                        let lower = line.to_lowercase();
                        for token in banned {
                            if lower.contains(token) {
                                hits.push(format!("{}:{}: {token}", path.display(), i + 1));
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "vendor ontology outside adapter zone:\n{}",
        hits.join("\n")
    );
}

// ---------------------------------------------------------------------------
// Unused-import guard (keeps the battery honest about its own surface)
// ---------------------------------------------------------------------------

#[test]
fn response_kind_vocabulary_is_stable() {
    assert_eq!(
        serde_json::to_string(&AgentResponseKind::Refusal).unwrap(),
        "\"refusal\""
    );
    let _: AgentResponse = serde_json::from_value(serde_json::json!({
        "kind": "refusal",
        "message": "no",
        "proposed_actions": [],
        "references": [],
        "uncertainty": null
    }))
    .unwrap();
}
