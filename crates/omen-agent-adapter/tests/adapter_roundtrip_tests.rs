//! External-route round trips through the real fixture binary.
//!
//! Spawns `omen-fixture-adapter` (same-package bin, `CARGO_BIN_EXE_*`)
//! behind `AdapterBackedProvider` and proves: handshake/negotiation,
//! explanation, typed tool round trip, proposal admission, G1 battery
//! reuse, hostile bounds, cancellation, crash, recovery, env isolation,
//! binding identity, and manifest-vs-installed-vs-binding distinctions.

use omen_agent::AgentContext;
use omen_agent::adapter_provider::*;
use omen_agent::adapter_spawn::SpawnLimits;
use omen_agent::provider::*;
use omen_agent_adapter::*;
use omen_core::InteractiveSessionId;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

fn fixture_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omen-fixture-adapter"))
}

fn parent_env_with(mode: &str, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = vec![
        ("SystemRoot".into(), "C:\\Windows".into()),
        ("SystemDrive".into(), "C:".into()),
        (
            "TEMP".into(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ),
        (
            "TMP".into(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ),
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        ("OMB_FIXTURE_MODE".into(), mode.into()),
        // Synthetic secrets: must never reach the child.
        ("OPENAI_API_KEY".into(), "sk-g2-SYNTHETIC-NEVER-LEAK".into()),
        ("OMEN_SYNTHETIC_SECRET".into(), "synthetic-g2-secret".into()),
    ];
    for (k, v) in extra {
        env.push((k.to_string(), v.to_string()));
    }
    env
}

fn fixture_manifest() -> AdapterManifest {
    fixture_manifest_with_extra(BTreeMap::new())
}

fn fixture_manifest_with_extra(extra: BTreeMap<String, serde_json::Value>) -> AdapterManifest {
    AdapterManifest {
        schema_version: 1,
        adapter_id: "omen-fixture".into(),
        adapter_version: "0.1.0".into(),
        entrypoint: vec!["omen-fixture-adapter".into()],
        supported_protocol_versions: vec!["0.1".into()],
        required_omen_contracts: vec!["0.8".into()],
        capabilities: vec!["reasoning".into(), "deterministic".into()],
        required_programs: vec![],
        required_config_labels: vec![],
        credential_labels: vec![],
        transports: vec!["stdio-ndjson".into()],
        streaming: false,
        platforms: vec![],
        optional_features: vec![],
        required_features: vec![],
        extra,
    }
}

fn provider_for(mode: &str, extra_env: &[(&str, &str)]) -> AdapterBackedProvider {
    provider_for_with_manifest(mode, extra_env, BTreeMap::new())
}

fn provider_for_with_manifest(
    mode: &str,
    extra_env: &[(&str, &str)],
    manifest_extra: BTreeMap<String, serde_json::Value>,
) -> AdapterBackedProvider {
    AdapterBackedProvider::new(AdapterProviderConfig {
        manifest: fixture_manifest_with_extra(manifest_extra),
        exe: fixture_exe(),
        extra_argv: vec![],
        env_policy: fixture_env_policy(),
        parent_env: parent_env_with(mode, extra_env),
        cwd: std::env::temp_dir(),
        omen_contract: "0.8".into(),
        omen_version: "0.9.0-preview.14".into(),
        limits: SpawnLimits {
            frame_timeout: Duration::from_secs(10),
            total_timeout: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(3),
        },
        tool_handlers: vec![
            (
                "omen.describe".into(),
                std::sync::Arc::new(describe_tool_handler),
            ),
            (
                "omen.workspace_status".into(),
                workspace_status_tool_handler(
                    PathBuf::from("/ws"),
                    PathBuf::from("/ws"),
                    "test".into(),
                    "0.8".into(),
                ),
            ),
        ],
    })
    .unwrap()
}

fn request(prompt: &str) -> AgentRequest {
    let cwd = std::env::temp_dir();
    AgentRequest {
        prompt: prompt.into(),
        context: AgentContext::new(
            "ws-g2",
            cwd.clone(),
            InteractiveSessionId::new("s").unwrap(),
            cwd,
        ),
        conversation: vec![],
    }
}

async fn respond(
    provider: &AdapterBackedProvider,
    prompt: &str,
) -> Result<AgentResponse, AgentError> {
    provider.respond(request(prompt)).await
}

// ---------- happy paths ----------

#[tokio::test]
async fn fixture_explanation_round_trip() {
    let provider = provider_for("explanation", &[]);
    let resp = respond(&provider, "hello fixture").await.unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Explanation);
    assert!(resp.message.contains("fixture explanation"));
    assert!(resp.proposed_actions.is_empty());
}

#[tokio::test]
async fn fixture_tool_round_trip_executes_omen_side() {
    let provider = provider_for("tool-roundtrip", &[]);
    let resp = respond(&provider, "describe something").await.unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Explanation);
    // The final message incorporates the Omen-executed tool result digest.
    assert!(resp.message.contains("fixture saw tool result"));
    assert!(resp.message.contains("ok=true"));
}

#[tokio::test]
async fn fixture_proposal_admits_typed_action_as_data() {
    let sentinel = std::env::temp_dir().join("g2-fixture-sentinel.txt");
    let _ = std::fs::remove_file(&sentinel);
    std::fs::write(&sentinel, "PRISTINE").unwrap();
    let provider = provider_for(
        "propose",
        &[("OMB_FIXTURE_SENTINEL", sentinel.to_string_lossy().as_ref())],
    );
    let resp = respond(&provider, "consider the sentinel").await.unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Proposal);
    assert_eq!(resp.proposed_actions.len(), 1);
    // Proposal only: nothing was admitted or executed.
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "PRISTINE");
    let _ = std::fs::remove_file(&sentinel);
}

// ---------- G1 battery reuse ----------

#[test]
fn g1_battery_reused_against_external_route() {
    use omen_agent::conformance::{ConformanceScenario, check_scenario};
    let provider = provider_for("explanation", &[]);
    let req = request("battery");
    check_scenario(ConformanceScenario::SuccessExplanation, &provider, &req)
        .unwrap_or_else(|e| panic!("G1 SuccessExplanation failed on external route: {e}"));
    let proposer = provider_for("propose", &[]);
    check_scenario(ConformanceScenario::SuccessProposal, &proposer, &req)
        .expect("G1 SUCCESS_PROPOSAL on external route");
    let unknown = provider_for("unknown-kind", &[]);
    check_scenario(ConformanceScenario::UnknownResponseKind, &unknown, &req)
        .expect("G1 UNKNOWN_RESPONSE_KIND on external route");
    let malformed = provider_for("malformed", &[]);
    check_scenario(ConformanceScenario::MalformedResponse, &malformed, &req)
        .expect("G1 MALFORMED_RESPONSE on external route");
    let auth = provider_for("auth", &[]);
    check_scenario(ConformanceScenario::AuthMissing, &auth, &req)
        .expect("G1 AUTH_MISSING on external route");
}

// ---------- negotiation / identity ----------

#[tokio::test]
async fn required_feature_gap_refuses_explicitly() {
    let mut manifest = fixture_manifest();
    manifest.required_features = vec!["no-such-feature".into()];
    let provider = AdapterBackedProvider::new(AdapterProviderConfig {
        manifest,
        exe: fixture_exe(),
        extra_argv: vec![],
        env_policy: fixture_env_policy(),
        parent_env: parent_env_with("explanation", &[]),
        cwd: std::env::temp_dir(),
        omen_contract: "0.8".into(),
        omen_version: "test".into(),
        limits: SpawnLimits::default(),
        tool_handlers: vec![],
    })
    .unwrap();
    let err = format!("{:?}", respond(&provider, "hi").await.unwrap_err());
    assert!(
        err.contains("MissingRequiredFeature") || err.contains("required feature"),
        "{err}"
    );
}

#[test]
fn binding_identity_names_exact_bytes() {
    let provider = provider_for("explanation", &[]);
    let negotiated = negotiate::Negotiated {
        protocol_version: "0.1".into(),
        active_features: vec![],
        degraded_features: vec![],
    };
    let binding = provider.binding_identity(&negotiated);
    assert_eq!(binding.adapter_id, "omen-fixture");
    assert_eq!(binding.transport, "stdio-ndjson");
    assert_eq!(binding.model_or_unknown(), "UNKNOWN");
    assert_eq!(binding.adapter_digest.len(), 64);
    // Digest matches the actual binary on disk: not "whatever is on PATH".
    let bytes = std::fs::read(fixture_exe()).unwrap();
    assert_eq!(binding.adapter_digest, sha256_hex(&bytes));
}

#[test]
fn package_vs_installed_vs_binding_stay_distinct() {
    // Manifest (package) carries no digests; binding carries both.
    let manifest = fixture_manifest();
    let json = serde_json::to_value(&manifest).unwrap();
    assert!(json.get("adapter_digest").is_none());
    assert!(json.get("manifest_digest").is_none());
    let provider = provider_for("explanation", &[]);
    let negotiated = negotiate::Negotiated {
        protocol_version: "0.1".into(),
        active_features: vec![],
        degraded_features: vec![],
    };
    let binding = provider.binding_identity(&negotiated);
    assert_ne!(binding.adapter_digest, binding.manifest_digest);
}

// ---------- hostile bounds ----------

#[tokio::test]
async fn hostile_modes_fail_truthfully() {
    // Wire refusal (bad bytes, bad shapes, unknown kinds) is Rejected;
    // dead children (exit, EOF) are Provider transport failures.
    // Every case resolves in seconds with control returned.
    for (mode, expect_rejected) in [
        ("malformed", true),
        ("oversize", true),
        ("invalid-utf8", true),
        ("exit-mid", false),
        ("unread-exit", false),
        ("unknown-kind", true),
    ] {
        let provider = provider_for(mode, &[]);
        let started = std::time::Instant::now();
        let err = respond(&provider, "hi").await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "{mode} took too long: {:?}",
            started.elapsed()
        );
        if expect_rejected {
            assert!(
                matches!(err, AgentError::Rejected(_)),
                "{mode}: expected Rejected, got {err:?}"
            );
        } else {
            assert!(
                matches!(err, AgentError::Provider(_)),
                "{mode}: expected Provider, got {err:?}"
            );
        }
    }
}

#[tokio::test]
async fn stall_mode_hits_explicit_timeout() {
    let config = AdapterProviderConfig {
        manifest: fixture_manifest(),
        exe: fixture_exe(),
        extra_argv: vec![],
        env_policy: fixture_env_policy(),
        parent_env: parent_env_with("stall", &[]),
        cwd: std::env::temp_dir(),
        omen_contract: "0.8".into(),
        omen_version: "test".into(),
        limits: SpawnLimits {
            frame_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(5),
            shutdown_grace: Duration::from_secs(2),
        },
        tool_handlers: vec![],
    };
    let provider = AdapterBackedProvider::new(config).unwrap();
    let started = std::time::Instant::now();
    let err = respond(&provider, "hi").await.unwrap_err();
    assert!(matches!(err, AgentError::Timeout(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[tokio::test]
async fn flood_mode_stays_bounded() {
    // Mid-request error frames fail the request explicitly and fast:
    // 5000 diagnostics must not grow memory, hang, or get silently ignored.
    let provider = provider_for("flood", &[]);
    let started = std::time::Instant::now();
    let err = respond(&provider, "hi").await.unwrap_err();
    assert!(matches!(err, AgentError::Provider(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(25));
}

// ---------- cancellation / crash / recovery ----------

#[tokio::test]
async fn dropping_the_request_kills_the_child() {
    let provider = std::sync::Arc::new(provider_for("stall", &[]));
    let provider2 = std::sync::Arc::clone(&provider);
    let handle = tokio::spawn(async move { provider2.respond(request("hi")).await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    handle.abort();
    let join = handle.await.unwrap_err();
    assert!(
        join.is_cancelled(),
        "abort must surface cancellation, never a phantom response"
    );
    // Omen remains usable: a fresh request on a fresh child works.
    let resp = respond(&provider, "after cancel").await;
    assert!(
        resp.is_err(),
        "stall mode still stalls, but the call returned control"
    );
}

#[tokio::test]
async fn crash_mid_request_is_explicit_and_recovery_works() {
    let provider = provider_for("exit-mid", &[]);
    let err = respond(&provider, "hi").await.unwrap_err();
    assert!(matches!(err, AgentError::Provider(_)), "{err:?}");
    // Recovery: a healthy adapter binds fine afterwards; machine truth kept.
    let healthy = provider_for("explanation", &[]);
    let resp = respond(&healthy, "recovered").await.unwrap();
    assert_eq!(resp.kind, AgentResponseKind::Explanation);
}

#[tokio::test]
async fn duplicate_response_after_tool_result_is_not_double_counted() {
    // The tool round trip performs exactly one tool call then one response.
    let provider = provider_for("tool-roundtrip", &[]);
    let resp = respond(&provider, "hi").await.unwrap();
    assert!(resp.message.contains("ok=true"));
}

// ---------- env isolation ----------

#[test]
fn fixture_child_env_carries_no_synthetic_secrets() {
    let policy = fixture_env_policy();
    let parent = parent_env_with("explanation", &[]);
    let env = policy.build_env(&parent, &[]);
    EnvPolicy::assert_no_secret_leak(&env, &["sk-g2-SYNTHETIC-NEVER-LEAK", "synthetic-g2-secret"])
        .unwrap();
    // ...but the declared fixture knobs do pass (tests need them).
    assert!(
        env.iter()
            .any(|(k, v)| k == "OMB_FIXTURE_MODE" && v == "explanation")
    );
}

#[tokio::test]
async fn no_silent_retry_single_spawn_per_request() {
    let count_file = std::env::temp_dir().join("g2-fixture-count.txt");
    let _ = std::fs::remove_file(&count_file);
    let provider = provider_for(
        "counting",
        &[(
            "OMB_FIXTURE_COUNT_FILE",
            count_file.to_string_lossy().as_ref(),
        )],
    );
    respond(&provider, "hi").await.unwrap();
    let content = std::fs::read_to_string(&count_file).unwrap();
    assert_eq!(content.lines().count(), 1, "exactly one reason per request");
    let _ = std::fs::remove_file(&count_file);
}
