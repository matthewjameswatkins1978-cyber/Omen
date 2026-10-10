//! Live-egress gate proofs: ordinary test processes stay hermetic even
//! when fully configured with (synthetic) credentials and the real
//! production transport.
//!
//! Nothing in this file admits the process except the ignored live smoke
//! test (which additionally requires `OMEN_LIVE_TESTS=1`). Every other
//! test asserts fail-closed refusal: a leak becomes a loud failure,
//! never a silent live request.
//!
//! Keys below are synthetic fixtures (`…-not-real`), never real
//! credentials. They prove the gate blocks emission even when the
//! provider is fully configured — the exact shape of the original
//! ambient-credential leak.

use omen_agent::{
    ANTHROPIC_SONNET_MODEL, ANTHROPIC_SONNET_PROVIDER_ID, AgentContext, AgentError, AgentProvider,
    AgentRequest, AgentResponseKind, AgentTurn, AnthropicConfig, AnthropicSonnetProvider,
    OPENAI_LUNA_PROVIDER_ID, OpenAiResponsesProvider, ProviderRegistry, openai_luna_config,
};
use omen_core::InteractiveSessionId;
use std::path::PathBuf;

fn sample_request(prompt: &str) -> AgentRequest {
    AgentRequest {
        prompt: prompt.into(),
        context: AgentContext::new(
            "ws",
            PathBuf::from("/ws"),
            InteractiveSessionId::new("s1").unwrap(),
            PathBuf::from("/ws"),
        ),
        conversation: vec![AgentTurn {
            role: "user".into(),
            text: "hello".into(),
            timestamp: "2026-09-22T00:00:00Z".into(),
        }],
    }
}

fn luna_with_real_transport() -> OpenAiResponsesProvider {
    OpenAiResponsesProvider::with_config(
        openai_luna_config(),
        Some("sk-luna-hermetic-fixture-not-real".to_string()),
        OPENAI_LUNA_PROVIDER_ID,
    )
}

fn sonnet_with_real_transport() -> AnthropicSonnetProvider {
    AnthropicSonnetProvider::with_config(
        AnthropicConfig::new(ANTHROPIC_SONNET_MODEL),
        Some("sk-ant-hermetic-fixture-not-real".to_string()),
        ANTHROPIC_SONNET_PROVIDER_ID,
    )
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// Configured Luna over the REAL production transport refuses without admission.
#[test]
fn luna_real_transport_refuses_without_admission() {
    assert!(
        !omen_agent::is_admitted(),
        "ordinary test processes must never be admitted"
    );
    let provider = luna_with_real_transport();
    assert!(
        provider.has_credential(),
        "fixture must be fully configured: the gate, not absence, blocks"
    );
    let err = block_on(provider.respond(sample_request("? why did the build fail")))
        .expect_err("live egress without admission must fail, never succeed");
    let rendered = format!("{err:?} {err}");
    assert!(
        rendered.contains("live network refused"),
        "refusal must carry the fail-closed signal, got: {rendered}"
    );
    assert!(
        !rendered.contains("sk-luna-hermetic-fixture-not-real"),
        "refusal must never echo credential material"
    );
}

/// Configured Sonnet over the REAL production transport refuses without admission.
#[test]
fn sonnet_real_transport_refuses_without_admission() {
    assert!(!omen_agent::is_admitted());
    let provider = sonnet_with_real_transport();
    assert!(provider.has_credential());
    let err = block_on(provider.respond(sample_request("? why did the build fail")))
        .expect_err("live egress without admission must fail, never succeed");
    let rendered = format!("{err:?} {err}");
    assert!(
        rendered.contains("live network refused"),
        "refusal must carry the fail-closed signal, got: {rendered}"
    );
    assert!(!rendered.contains("sk-ant-hermetic-fixture-not-real"));
}

/// Refusal is fail-closed, not a silent fallback: the caller gets an
/// error shaped for the lane's "Reasoning unavailable" path, and the
/// diagnostic identity still names the intended provider/transport.
#[test]
fn refusal_is_fail_closed_not_fallback() {
    let provider = luna_with_real_transport();
    let identity = provider.provider_identity();
    assert_eq!(identity.id, "openai-luna");
    assert_eq!(identity.model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(identity.transport.as_deref(), Some("live-https"));
    let err = block_on(provider.respond(sample_request("? status")))
        .expect_err("must be Err, never an Ok fallback");
    assert!(
        matches!(err, AgentError::Provider(_)),
        "refusal must be a provider error (lane renders Reasoning unavailable), got: {err:?}"
    );
}

/// Registry built with a synthetic ambient credential registers Luna as
/// default (production selection preserved) — while the gate still
/// refuses emission in this unadmitted process.
#[test]
fn configured_registry_selects_luna_but_gate_still_refuses() {
    let _lock = ambient_lock();
    let _env = AmbientEnv::set("OPENAI_API_KEY", "sk-luna-hermetic-fixture-not-real");
    let registry = ProviderRegistry::new();
    assert_eq!(
        registry.active_descriptor().id,
        "openai-luna",
        "Luna stays the default when configured (production behavior)"
    );
    let active = registry.active_provider();
    let err = block_on(active.respond(sample_request("? status"))).expect_err("gate refuses");
    let rendered = format!("{err:?} {err}");
    assert!(rendered.contains("live network refused"));
}

/// Process-env guard with restore-on-drop for the ambient-credential proof.
struct AmbientEnv {
    key: &'static str,
    previous: Option<String>,
}

impl AmbientEnv {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for AmbientEnv {
    fn drop(&mut self) {
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var(self.key, v.clone()),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

static AMBIENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ambient_lock() -> std::sync::MutexGuard<'static, ()> {
    AMBIENT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Explicit live smoke test: ignored by default, requires
/// `OMEN_LIVE_TESTS=1` AND this deliberate admission. Bounds spend to a
/// single attempt with the standard 800-token cap and asserts observable
/// usage. Run by a human approver only; CI never runs it.
///
/// Example: `OMEN_LIVE_TESTS=1 cargo test -p omen-agent --test
/// live_gate_tests -- --ignored live_smoke_bounded_request`
#[test]
#[ignore]
fn live_smoke_bounded_request() {
    omen_agent::admit_for_explicit_live_test();
    let provider = omen_agent::openai_luna_provider();
    if !provider.has_credential() {
        panic!("live smoke needs a real configured credential; none found (no spend attempted)");
    }
    let started = std::time::Instant::now();
    let response =
        block_on(provider.respond(sample_request("Reply with one word: ok."))).expect("live round");
    let latency = started.elapsed();
    assert_eq!(response.kind, AgentResponseKind::Explanation);
    let usage = response
        .usage
        .as_ref()
        .expect("live validation must report observable usage");
    assert!(
        usage.input_tokens > 0,
        "usage must be measured, not invented"
    );
    assert!(response.latency_ms.is_some());
    assert!(
        !format!("{response:?}").contains("live network refused"),
        "admitted process must reach the API"
    );
    assert!(latency.as_secs() < 60, "bounded smoke must stay quick");
}
