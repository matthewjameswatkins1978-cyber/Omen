//! G1 provider-switching truth at the session / AI-lane level.
//!
//! Proves end-to-end (registry selection -> session -> lane dispatch) that:
//! displayed provider == actually invoked provider; failed switches preserve
//! the active provider; unavailable providers never trigger silent fallback;
//! deterministic truth bypasses every provider; one request is one call;
//! authenticated proposals are data, never executions; availability is not
//! admission; disconnect/recovery leaves no poisoned state.
//!
//! All providers here are deterministic (canary, diagnostic, counted mocks).
//! Zero network, zero credits, zero sleeps.

use omen_agent::{
    AgentProvider, ConformanceMode, ConformanceProvider, HttpPost, HttpResponseSnapshot,
    OpenAiResponsesProvider, ProviderDescriptor, openai_luna_config,
};
use omen_interactive::InteractiveSession;
use omen_interactive::ai_lane::AiLaneDispatcher;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::tempdir;

/// Reasoning query proven to route to the provider (not the deterministic
/// classifier): the live lane proof asserts provider_calls == 1 for it.
const REASONING_QUERY: &str =
    "explain why exit code 2 does not necessarily mean Omen itself failed";
const DETERMINISTIC_QUERY: &str = "? what folder am I in?";

struct CountingMock {
    status: u16,
    body: String,
    calls: AtomicUsize,
}

impl CountingMock {
    fn ok(body: &str) -> Self {
        Self {
            status: 200,
            body: body.into(),
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
        Ok(HttpResponseSnapshot {
            status: self.status,
            retry_after_secs: None,
            body: self.body.clone(),
        })
    }
}

fn mock_envelope(message: &str) -> String {
    let inner = serde_json::json!({
        "kind": "explanation",
        "message": message,
        "proposed_actions": [],
        "references": [],
        "uncertainty": null,
    });
    serde_json::json!({
        "status": "completed",
        "output": [{
            "type": "message",
            "content": [{ "type": "output_text", "text": inner.to_string() }],
        }],
    })
    .to_string()
}

fn canary_descriptor(id: &str) -> ProviderDescriptor {
    ProviderDescriptor {
        id: id.into(),
        name: format!("G1 test canary {id}"),
        model: Some("deterministic-canary".into()),
        credential_source: Some("none".into()),
        capabilities: vec!["reasoning".into(), "proposal".into()],
        is_available: true,
    }
}

fn mock_luna_descriptor() -> ProviderDescriptor {
    ProviderDescriptor {
        id: "mock-luna".into(),
        name: "G1 mocked Luna".into(),
        model: Some("gpt-6-luna".into()),
        credential_source: Some("environment:OPENAI_API_KEY".into()),
        capabilities: vec!["reasoning".into(), "structured-response".into()],
        is_available: true,
    }
}

fn mock_luna(mock: CountingMock) -> (Arc<dyn AgentProvider>, Arc<CountingMock>) {
    let mock = Arc::new(mock);
    let provider: Arc<dyn AgentProvider> = Arc::new(OpenAiResponsesProvider::with_http(
        openai_luna_config(),
        Some("sk-g1-synthetic".to_string()),
        mock.clone(),
        "mock-luna",
    ));
    (provider, mock)
}

fn dispatch(session: &InteractiveSession, query: &str) -> omen_interactive::ai_lane::AiLaneOutput {
    AiLaneDispatcher::dispatch_with_session(
        query,
        &session.session_id,
        session.cwd.as_path(),
        session.db.as_ref(),
        session.agent_provider.as_deref(),
        Some(&session.comp_ctx),
    )
    .expect("lane dispatch must not error at the transport layer")
}

fn new_session() -> (tempfile::TempDir, InteractiveSession) {
    let dir = tempdir().unwrap();
    let session = InteractiveSession::new(dir.path().to_path_buf(), None).unwrap();
    (dir, session)
}

// ---------------------------------------------------------------------------
// A/B/C: switching coherence — descriptor, status text, and invoked instance
// ---------------------------------------------------------------------------

#[test]
fn switching_coherence_diagnostic_canarya_canaryb_diagnostic() {
    let (_dir, mut session) = new_session();
    assert_eq!(session.agent_registry.active_descriptor().id, "diagnostic");

    let canary_a = Arc::new(ConformanceProvider::new(
        ConformanceMode::SuccessExplanation,
    ));
    let canary_b = Arc::new(ConformanceProvider::new(
        ConformanceMode::SuccessExplanation,
    ));
    session.register_agent_provider(canary_descriptor("canary-a"), canary_a.clone());
    session.register_agent_provider(canary_descriptor("canary-b"), canary_b.clone());

    // A: select canary-a; status, descriptor, and invoked instance agree.
    session.use_agent_provider("canary-a").unwrap();
    let out = dispatch(&session, REASONING_QUERY);
    assert!(out.configured);
    assert_eq!(out.stats.provider_calls, 1);
    assert_eq!(canary_a.call_count(), 1);
    assert_eq!(canary_b.call_count(), 0);
    assert_eq!(session.agent_registry.active_descriptor().id, "canary-a");
    assert!(
        session.agent_registry.status_text().contains("canary-a"),
        "status must name the invoked provider"
    );

    // B: switch to canary-b; invocation follows selection exactly.
    session.use_agent_provider("canary-b").unwrap();
    let out = dispatch(&session, REASONING_QUERY);
    assert!(out.configured);
    assert_eq!(canary_a.call_count(), 1, "canary-a must not be re-invoked");
    assert_eq!(canary_b.call_count(), 1);
    assert_eq!(session.agent_registry.active_descriptor().id, "canary-b");

    // C: switch back to diagnostic; the built-in answers, canaries stay still.
    session.use_agent_provider("diagnostic").unwrap();
    let out = dispatch(&session, REASONING_QUERY);
    assert!(out.configured);
    assert_eq!(out.stats.provider_calls, 1);
    assert!(
        out.response_text.contains("shared Omen machine state"),
        "diagnostic must answer, got: {}",
        &out.response_text[..out.response_text.len().min(200)]
    );
    assert_eq!(canary_a.call_count(), 1);
    assert_eq!(canary_b.call_count(), 1, "canary-b invoked exactly once");
    assert_eq!(session.agent_registry.active_descriptor().id, "diagnostic");
}

#[test]
fn failed_switch_is_explicit_and_preserves_active() {
    let (_dir, mut session) = new_session();
    let canary_a = Arc::new(ConformanceProvider::new(
        ConformanceMode::SuccessExplanation,
    ));
    session.register_agent_provider(canary_descriptor("canary-a"), canary_a.clone());
    session.use_agent_provider("canary-a").unwrap();

    match session.use_agent_provider("no-such-provider") {
        Err(omen_agent::AgentError::ProviderUnavailable { .. }) => {}
        other => panic!("unknown provider must fail explicitly, got {other:?}"),
    }
    assert_eq!(session.agent_registry.active_descriptor().id, "canary-a");
    let out = dispatch(&session, REASONING_QUERY);
    assert!(out.configured);
    assert_eq!(canary_a.call_count(), 1);
}

// ---------------------------------------------------------------------------
// E: unavailable provider never triggers silent fallback
// ---------------------------------------------------------------------------

#[test]
fn unavailable_provider_fails_open_with_truth_not_fallback() {
    let (_dir, mut session) = new_session();
    let canary_a = Arc::new(ConformanceProvider::new(
        ConformanceMode::SuccessExplanation,
    ));
    let canary_b = Arc::new(ConformanceProvider::new(ConformanceMode::TransportFailure));
    session.register_agent_provider(canary_descriptor("canary-a"), canary_a.clone());
    session.register_agent_provider(canary_descriptor("canary-b"), canary_b.clone());
    session.use_agent_provider("canary-b").unwrap();

    let out = dispatch(&session, REASONING_QUERY);
    assert!(!out.configured, "failed provider must surface, not succeed");
    assert!(
        out.response_text.contains("Reasoning unavailable"),
        "failure must stay explicit, got: {}",
        &out.response_text[..out.response_text.len().min(300)]
    );
    assert!(out.proposed_actions.is_empty());
    assert_eq!(canary_b.call_count(), 1, "exactly one attempt, no retry");
    assert_eq!(
        canary_a.call_count(),
        0,
        "Omen must NOT silently substitute another provider"
    );
    // Status still names the requested provider: no lie about who answered.
    assert_eq!(session.agent_registry.active_descriptor().id, "canary-b");
}

// ---------------------------------------------------------------------------
// Deterministic first: provider identity never changes routing
// ---------------------------------------------------------------------------

#[test]
fn deterministic_queries_bypass_hostile_provider() {
    let (_dir, mut session) = new_session();
    let canary_a = Arc::new(ConformanceProvider::new(
        ConformanceMode::SuccessExplanation,
    ));
    session.register_agent_provider(canary_descriptor("canary-a"), canary_a.clone());
    session.use_agent_provider("canary-a").unwrap();

    let out = dispatch(&session, DETERMINISTIC_QUERY);
    assert_eq!(out.stats.provider_calls, 0);
    assert_eq!(
        canary_a.call_count(),
        0,
        "hostile provider must not be asked"
    );
    assert!(out.response_text.contains("You are in folder"));
}

#[test]
fn deterministic_queries_bypass_mocked_luna() {
    let (_dir, mut session) = new_session();
    let (luna, mock) = mock_luna(CountingMock::ok(&mock_envelope("mocked hello")));
    session.register_agent_provider(mock_luna_descriptor(), luna);
    session.use_agent_provider("mock-luna").unwrap();

    let out = dispatch(&session, DETERMINISTIC_QUERY);
    assert_eq!(out.stats.provider_calls, 0);
    assert_eq!(mock.call_count(), 0);

    // …while genuine reasoning still reaches the same provider exactly once.
    let out = dispatch(&session, REASONING_QUERY);
    assert!(out.configured);
    assert_eq!(out.stats.provider_calls, 1);
    assert_eq!(mock.call_count(), 1);
    assert!(out.response_text.contains("mocked hello"));
}

// ---------------------------------------------------------------------------
// No silent retries at the lane
// ---------------------------------------------------------------------------

#[test]
fn lane_makes_exactly_one_call_on_transport_failure() {
    let (_dir, mut session) = new_session();
    let canary = Arc::new(ConformanceProvider::new(ConformanceMode::TransportFailure));
    session.register_agent_provider(canary_descriptor("canary-retry"), canary.clone());
    session.use_agent_provider("canary-retry").unwrap();

    let out = dispatch(&session, REASONING_QUERY);
    assert!(!out.configured);
    assert_eq!(out.stats.provider_calls, 1);
    assert_eq!(
        canary.call_count(),
        1,
        "one user request must be one provider call"
    );
}

// ---------------------------------------------------------------------------
// Authentication != authority: proposals are data, never executions
// ---------------------------------------------------------------------------

#[test]
fn authenticated_proposal_grants_no_execution() {
    let (dir, mut session) = new_session();
    let sentinel = dir.path().join("g1-must-never-exist");
    let canary = Arc::new(ConformanceProvider::new(ConformanceMode::ProposalExec(
        vec!["touch".into(), sentinel.to_string_lossy().to_string()],
    )));
    session.register_agent_provider(canary_descriptor("canary-auth"), canary.clone());
    session.use_agent_provider("canary-auth").unwrap();
    let cwd_before = session.cwd.clone();

    let out = dispatch(&session, REASONING_QUERY);
    assert!(out.configured, "proposal is admitted as typed data");
    assert!(
        out.proposed_actions
            .iter()
            .any(|a| matches!(a, omen_agent::ProposedAction::ExecuteCommand { .. })),
        "ExecuteCommand proposal must be present as data"
    );
    assert!(
        out.response_text.contains("Proposed action"),
        "proposal must be presented, not run"
    );
    assert!(
        !sentinel.exists(),
        "provider proposal must NEVER execute (sentinel was created)"
    );
    assert_eq!(session.cwd, cwd_before, "shell location must be untouched");
    assert_eq!(canary.call_count(), 1);
}

// ---------------------------------------------------------------------------
// Availability != admission at the lane
// ---------------------------------------------------------------------------

#[test]
fn lane_rejects_malformed_output_from_reachable_provider() {
    let (_dir, mut session) = new_session();
    let (luna, mock) = mock_luna(CountingMock::ok("garbage{{{not json"));
    session.register_agent_provider(mock_luna_descriptor(), luna);
    session.use_agent_provider("mock-luna").unwrap();

    let out = dispatch(&session, REASONING_QUERY);
    assert!(!out.configured);
    assert!(
        out.response_text.contains("Reasoning unavailable"),
        "malformed output must be rejected, got: {}",
        &out.response_text[..out.response_text.len().min(300)]
    );
    assert!(out.response_text.contains("rejected") || out.response_text.contains("malformed"));
    assert_eq!(mock.call_count(), 1);
}

// ---------------------------------------------------------------------------
// Disconnect then recovery leaves no poisoned state
// ---------------------------------------------------------------------------

#[test]
fn lane_disconnect_recovery_sequence() {
    let (_dir, mut session) = new_session();
    let canary = Arc::new(ConformanceProvider::new(ConformanceMode::Script(
        VecDeque::from([
            ConformanceMode::TransportFailure,
            ConformanceMode::SuccessExplanation,
        ]),
    )));
    session.register_agent_provider(canary_descriptor("canary-flap"), canary.clone());
    session.use_agent_provider("canary-flap").unwrap();

    let first = dispatch(&session, REASONING_QUERY);
    assert!(!first.configured, "transport failure must surface");
    assert_eq!(canary.call_count(), 1);

    let second = dispatch(&session, REASONING_QUERY);
    assert!(second.configured, "provider must recover on next call");
    assert!(
        second.response_text.contains("canary explanation"),
        "normal response after recovery"
    );
    assert_eq!(canary.call_count(), 2);
    assert_eq!(session.agent_registry.active_descriptor().id, "canary-flap");
}
