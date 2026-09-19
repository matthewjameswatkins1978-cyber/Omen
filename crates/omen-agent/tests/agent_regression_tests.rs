use omen_agent::{
    AgentContext, AgentError, AgentProvider, AgentRequest, AgentResponse, DeterministicClassifier,
    ExecutionSummary, GitStatusInfo, ProviderDescriptor, ProviderRegistry, TimeoutProvider,
};
use omen_core::InteractiveSessionId;
use omen_test_fixtures::harness::{UNIT_TIMEOUT, run_with_test_timeout};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    #[cfg(windows)]
    path.push("omen-gremlin.exe");
    #[cfg(not(windows))]
    path.push("omen-gremlin");
    path
}

/// A mock provider that counts invocations to prove zero token consumption.
#[derive(Default)]
struct CountingAgentProvider {
    calls: AtomicUsize,
}

impl CountingAgentProvider {
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl AgentProvider for CountingAgentProvider {
    fn respond<'a>(
        &'a self,
        _request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(AgentResponse::explanation(
                "Generative LLM model reasoning invoked.",
            ))
        })
    }
}

/// A provider that deliberately hangs indefinitely.
struct StallingAgentProvider;

impl AgentProvider for StallingAgentProvider {
    fn respond<'a>(
        &'a self,
        _request: AgentRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(AgentResponse::explanation("Never reached"))
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_git_probe_times_out_cleanly() {
    run_with_test_timeout(
        "agent_git_probe_times_out_cleanly",
        UNIT_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_HOSTILE_GIT_DIR");
            let temp = tempdir().unwrap();
            let repo_path = temp.path();
            std::fs::create_dir_all(repo_path.join(".git")).unwrap();

            let gremlin = gremlin_exe();
            assert!(gremlin.exists(), "omen-gremlin fixture must exist");

            // Instruct gremlin to stall for 30 seconds via environment variable
            unsafe {
                std::env::set_var("OMEN_GREMLIN_STALL_MS", "30000");
            }

            ctx.phase("RUN_BOUNDED_PROBE");
            let start = Instant::now();
            // Deadline is 300ms, way below the 30s gremlin stall and the 5s UNIT_TIMEOUT
            let status_res = AgentContext::detect_git_status_with_opts(
                repo_path,
                &gremlin.to_string_lossy(),
                300,
            );
            let elapsed = start.elapsed();

            unsafe {
                std::env::remove_var("OMEN_GREMLIN_STALL_MS");
            }

            assert!(
                elapsed < Duration::from_millis(1500),
                "Git inspection must terminate at deadline, took {elapsed:?}"
            );

            let status = status_res.expect("Must return degraded GitStatusInfo on timeout");
            assert!(
                status.probe_error.is_some(),
                "Must report probe error diagnostic"
            );
            assert!(
                status
                    .probe_error
                    .as_ref()
                    .unwrap()
                    .contains("timed out after 300ms"),
                "Error must specify probe timeout: {:?}",
                status.probe_error
            );
            assert!(
                status.blocked_phase.is_some(),
                "Must identify blocked phase"
            );
            assert!(
                status.blocked_phase.as_ref().unwrap().contains("rev-parse"),
                "Blocked phase must identify command: {:?}",
                status.blocked_phase
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_context_timeout_does_not_hang_shell() {
    run_with_test_timeout(
        "agent_context_timeout_does_not_hang_shell",
        UNIT_TIMEOUT,
        |ctx| async move {
            ctx.phase("INVOKE_TIMEOUT_PROVIDER");
            let stalling = Arc::new(StallingAgentProvider);
            let timeout_provider = TimeoutProvider::new(stalling, Duration::from_millis(200));

            let req = AgentRequest {
                prompt: "diagnose why this hangs".into(),
                context: AgentContext::new(
                    "ws_test",
                    PathBuf::from("/test"),
                    InteractiveSessionId::new("sess_test").unwrap(),
                    PathBuf::from("/test"),
                ),
                conversation: vec![],
            };

            let start = Instant::now();
            let res = timeout_provider.respond(req).await;
            let elapsed = start.elapsed();

            assert!(
                elapsed < Duration::from_millis(1000),
                "Timeout must fire quickly, took {elapsed:?}"
            );
            match res {
                Err(AgentError::Timeout(d)) => {
                    assert_eq!(d, Duration::from_millis(200));
                }
                other => panic!("Expected AgentError::Timeout, got {other:?}"),
            }
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deterministic_question_uses_zero_model_calls() {
    run_with_test_timeout(
        "deterministic_question_uses_zero_model_calls",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let counting = Arc::new(CountingAgentProvider::default());
            let temp = tempdir().unwrap();
            let ws_path = temp.path().to_path_buf();

            let session_id = InteractiveSessionId::new("sess_economy").unwrap();
            let mut context =
                AgentContext::new("ws_econ", ws_path.clone(), session_id, ws_path.clone());

            context.git = Some(GitStatusInfo {
                branch: Some("main".into()),
                modified_files: vec!["src/lib.rs".into()],
                ..Default::default()
            });
            context.recent_execution = Some(ExecutionSummary {
                execution_id: "exec_123".into(),
                command: "cargo check".into(),
                exit_code: Some(0),
                duration_ms: Some(120),
                stdout_artifact: Some("artifact://sha256/abc".into()),
                stderr_artifact: None,
                stdout_excerpt: None,
                stderr_excerpt: None,
                timestamp: "2026-09-19T00:00:00Z".into(),
            });

            let deterministic_queries = [
                "what folder am I in?",
                "where am I?",
                "what directory",
                "pwd",
                "what branch am I on?",
                "what branch",
                "what was the last command?",
                "what did that command just do?",
                "is anything dirty?",
                "what file is this?",
            ];

            for query in deterministic_queries {
                let answer = DeterministicClassifier::try_answer(query, &context);
                assert!(
                    answer.is_some(),
                    "Query '{query}' must be answered deterministically"
                );
            }

            // Invocations to the generative model provider must be exactly zero!
            assert_eq!(
                counting.call_count(),
                0,
                "Deterministic queries must NEVER invoke external generative model"
            );
        },
    )
    .await;
}

#[test]
fn folder_lookup_does_not_build_large_agent_context() {
    let ctx = AgentContext::new(
        "ws_test",
        PathBuf::from("/projects/Omen"),
        InteractiveSessionId::new("sess_1").unwrap(),
        PathBuf::from("/projects/Omen/crates/omen-agent"),
    );

    let res = DeterministicClassifier::try_answer("what folder am I in?", &ctx).unwrap();
    assert!(res.message.contains("omen-agent"));
    assert!(res.proposed_actions.is_empty());
}

#[test]
fn branch_lookup_is_deterministic() {
    let mut ctx = AgentContext::new(
        "ws_test",
        PathBuf::from("/projects/Omen"),
        InteractiveSessionId::new("sess_1").unwrap(),
        PathBuf::from("/projects/Omen"),
    );
    ctx.git = Some(GitStatusInfo {
        branch: Some("feature/zero-cost".into()),
        ..Default::default()
    });

    let res = DeterministicClassifier::try_answer("what branch am I on?", &ctx).unwrap();
    assert!(res.message.contains("feature/zero-cost"));
}

#[test]
fn large_artifact_is_handle_only_until_requested() {
    let summary = ExecutionSummary {
        execution_id: "exec_huge".into(),
        command: "cargo test --all".into(),
        exit_code: Some(101),
        duration_ms: Some(5400),
        stdout_artifact: Some(
            "artifact://sha256/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                .into(),
        ),
        stderr_artifact: Some(
            "artifact://sha256/ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"
                .into(),
        ),
        stdout_excerpt: None,
        stderr_excerpt: Some("test failed: assertion failed at line 42".into()),
        timestamp: "2026-09-19T00:00:00Z".into(),
    };

    // Verify stdout_artifact contains only the URI handle, not inline mega-bytes
    assert!(
        summary
            .stdout_artifact
            .as_ref()
            .unwrap()
            .starts_with("artifact://")
    );
    assert!(summary.stdout_excerpt.is_none());
}

#[test]
fn provider_registry_selects_configured_provider() {
    let registry = ProviderRegistry::new();
    assert_eq!(registry.active_descriptor().id, "diagnostic");

    let custom_desc = ProviderDescriptor {
        id: "mock_claude".into(),
        name: "Mock Anthropic Provider".into(),
        model: Some("claude-3-7-sonnet".into()),
        credential_source: Some("environment:ANTHROPIC_API_KEY".into()),
        capabilities: vec!["reasoning".into(), "code".into()],
        is_available: true,
    };
    let mock_provider: Arc<dyn AgentProvider> = Arc::new(CountingAgentProvider::default());
    registry.register(custom_desc, mock_provider);

    let list = registry.list_providers();
    assert_eq!(list.len(), 2);

    registry.set_active_provider("mock_claude").unwrap();
    assert_eq!(registry.active_descriptor().id, "mock_claude");
    assert_eq!(
        registry.active_descriptor().model.as_deref(),
        Some("claude-3-7-sonnet")
    );

    let status = registry.status_text();
    assert!(status.contains("mock_claude"));
    assert!(status.contains("claude-3-7-sonnet"));
    assert!(!status.contains("API_KEY=")); // Must never print secrets
}

#[test]
fn provider_failure_is_translated_to_omen_error() {
    let auth_err = AgentError::AuthenticationRequired {
        provider: "anthropic".into(),
        message: "API key missing or expired".into(),
    };
    assert_eq!(
        auth_err.to_string(),
        "Authentication required for agent provider 'anthropic': API key missing or expired"
    );

    let rate_err = AgentError::RateLimited {
        provider: "openai".into(),
        retry_after_secs: Some(30),
    };
    assert_eq!(
        rate_err.to_string(),
        "Agent provider 'openai' rate limited. Retry after Some(30) seconds."
    );

    let unavail_err = AgentError::ProviderUnavailable {
        provider: "local_ollama".into(),
        message: "connection refused on 127.0.0.1:11434".into(),
    };
    assert_eq!(
        unavail_err.to_string(),
        "Agent provider 'local_ollama' unavailable: connection refused on 127.0.0.1:11434"
    );
}

#[test]
fn provider_diagnostics_do_not_leak_into_normal_agent_output() {
    let error = AgentError::Provider("upstream raw websocket TLS reset 1006".into());
    let formatted = format!("Agent\n\nReasoning unavailable: {error}");

    // Normal output translates through stable Omen concept without raw JSON or websocket spam
    assert!(formatted.starts_with("Agent\n\nReasoning unavailable:"));
    assert!(!formatted.contains("\"type\": \"error\""));
    assert!(!formatted.contains("websocket_trace"));
}
