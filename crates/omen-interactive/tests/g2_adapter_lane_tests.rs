//! G2 lane proofs: an out-of-process adapter behind the registry.
//!
//! The fixture binary is a same-workspace bin; it is located via the cargo
//! target directory (integration tests cannot use another crate's
//! `CARGO_BIN_EXE_*`). `cargo test --workspace --all-targets` builds it.

use omen_agent::adapter_provider::{
    AdapterBackedProvider, AdapterProviderConfig, describe_tool_handler,
    workspace_status_tool_handler,
};
use omen_agent::adapter_spawn::SpawnLimits;
use omen_agent::codex::{CODEX_PROVIDER_ID, codex_descriptor, resolve_codex_exe};
use omen_agent::provider::AgentProvider;
use omen_agent::registry::ProviderDescriptor;
use omen_agent_adapter::{AdapterManifest, fixture_env_policy};
use omen_interactive::ai_lane::AiLaneDispatcher;
use omen_interactive::session::InteractiveSession;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const DETERMINISTIC_QUERY: &str = "? what folder am I in?";
const REASONING_QUERY: &str =
    "explain why exit code 2 does not necessarily mean Omen itself failed";

fn workspace_fixture_exe() -> PathBuf {
    // target/debug/deps/<test> -> target/debug/omen-fixture-adapter[.exe]
    let own = std::env::current_exe().expect("test exe path");
    let deps = own.parent().expect("deps dir").to_path_buf();
    let debug = if deps.file_name().map(|n| n == "deps").unwrap_or(false) {
        deps.parent().expect("target dir").to_path_buf()
    } else {
        deps
    };
    debug.join(format!(
        "omen-fixture-adapter{}",
        std::env::consts::EXE_SUFFIX
    ))
}

fn external_descriptor(id: &str) -> ProviderDescriptor {
    ProviderDescriptor {
        id: id.into(),
        name: format!("G2 external adapter {id}"),
        model: None,
        credential_source: Some("none".into()),
        capabilities: vec!["reasoning".into(), "deterministic".into()],
        is_available: true,
    }
}

fn external_provider(
    mode: &str,
    extra_env: &[(&str, &str)],
    count_file: Option<&std::path::Path>,
) -> Arc<dyn AgentProvider> {
    let exe = workspace_fixture_exe();
    assert!(
        exe.is_file(),
        "fixture binary missing at {} (build with --all-targets)",
        exe.display()
    );
    let mut parent_env = vec![
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
    ];
    for (k, v) in extra_env {
        parent_env.push((k.to_string(), v.to_string()));
    }
    if let Some(path) = count_file {
        parent_env.push((
            "OMB_FIXTURE_COUNT_FILE".into(),
            path.to_string_lossy().into_owned(),
        ));
    }
    let manifest = AdapterManifest {
        schema_version: 1,
        adapter_id: "omen-fixture".into(),
        adapter_version: "0.1.0".into(),
        entrypoint: vec!["omen-fixture-adapter".into()],
        supported_protocol_versions: vec!["0.1".into()],
        required_omen_contracts: vec!["0.8".into()],
        capabilities: vec!["reasoning".into()],
        required_programs: vec![],
        required_config_labels: vec![],
        credential_labels: vec![],
        transports: vec!["stdio-ndjson".into()],
        streaming: false,
        platforms: vec![],
        optional_features: vec![],
        required_features: vec![],
        extra: Default::default(),
    };
    let provider = AdapterBackedProvider::new(AdapterProviderConfig {
        manifest,
        exe,
        extra_argv: vec![],
        env_policy: fixture_env_policy(),
        parent_env,
        cwd: std::env::temp_dir(),
        omen_contract: "0.8".into(),
        omen_version: "test".into(),
        limits: SpawnLimits {
            frame_timeout: Duration::from_secs(10),
            total_timeout: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(3),
        },
        tool_handlers: vec![
            ("omen.describe".into(), Arc::new(describe_tool_handler)),
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
    .unwrap();
    Arc::new(provider)
}

fn new_session() -> (tempfile::TempDir, InteractiveSession) {
    let dir = tempfile::tempdir().unwrap();
    let session = InteractiveSession::new(dir.path().to_path_buf(), None).unwrap();
    (dir, session)
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

#[test]
fn deterministic_query_makes_zero_external_adapter_calls() {
    let (_dir, mut session) = new_session();
    let count_file = std::env::temp_dir().join("g2-lane-count.txt");
    let _ = std::fs::remove_file(&count_file);
    let ext = external_provider("counting", &[], Some(&count_file));
    session.register_agent_provider(external_descriptor("ext-fixture"), ext);
    session.use_agent_provider("ext-fixture").unwrap();

    let out = dispatch(&session, DETERMINISTIC_QUERY);
    let text = format!("{out:?}");
    assert!(
        text.contains("Temp") || text.to_lowercase().contains("folder"),
        "deterministic answer missing: {text}"
    );
    let calls = std::fs::read_to_string(&count_file)
        .map(|c| c.lines().count())
        .unwrap_or(0);
    assert_eq!(
        calls, 0,
        "deterministic path must bypass the external adapter"
    );
    let _ = std::fs::remove_file(&count_file);
}

#[test]
fn external_adapter_switching_coherence_at_lane() {
    let (_dir, mut session) = new_session();
    let ext = external_provider("explanation", &[], None);
    session.register_agent_provider(external_descriptor("ext-a"), ext);
    session.use_agent_provider("ext-a").unwrap();

    assert_eq!(session.agent_registry.active_descriptor().id, "ext-a");
    let status = session.agent_registry.status_text();
    assert!(status.contains("Provider: ext-a"), "{status}");

    let out = dispatch(&session, REASONING_QUERY);
    assert_eq!(out.stats.provider_calls, 1);
    assert!(
        out.response_text.contains("fixture explanation"),
        "got: {}",
        &out.response_text[..out.response_text.len().min(300)]
    );

    // Back to built-in: no residue, no fallback confusion.
    session.use_agent_provider("diagnostic").unwrap();
    assert_eq!(session.agent_registry.active_descriptor().id, "diagnostic");
}

#[test]
fn failed_external_switch_preserves_active() {
    let (_dir, session) = new_session();
    assert!(
        session
            .agent_registry
            .set_active_provider("no-such-adapter")
            .is_err()
    );
    assert_eq!(session.agent_registry.active_descriptor().id, "diagnostic");
}

#[test]
fn codex_registration_matches_path_without_spawning() {
    let (_dir, session) = new_session();
    let on_path = resolve_codex_exe().is_some();
    let listed = session
        .agent_registry
        .list_providers()
        .iter()
        .any(|d| d.id == CODEX_PROVIDER_ID);
    assert_eq!(
        listed, on_path,
        "codex must be listed exactly when its binary resolves on PATH (filesystem-only, no spawn)"
    );
    if on_path {
        let desc = codex_descriptor(true);
        assert_eq!(desc.model, None);
    }
}
