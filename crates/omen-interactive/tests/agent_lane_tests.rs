use omen_core::{Assurance, ExecutionId, InteractiveSessionId, ResourceUri};
use omen_daemon::DaemonServer;
use omen_interactive::resolver::ReferenceResolver;
use omen_interactive::session::InteractiveSession;
use omen_ipc::PlatformStream;
use omen_knowledge::{
    ContentAddressedStore, Database, ExecutionHistory, ExecutionRecord, FactRegistry,
    PublishFactRequest, canonical_workspace_db_path,
};
use omen_test_fixtures::{INTEGRATION_TIMEOUT, UNIT_TIMEOUT, run_with_test_timeout};
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_lane_im_lost_proof_a() {
    run_with_test_timeout(
        "test_agent_lane_im_lost_proof_a",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            // Initialize a git repo
            let _ = std::process::Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(ws_path)
                .output();

            fs::write(
                ws_path.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\n",
            )
            .unwrap();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let session_id = InteractiveSessionId::new("sess-proof-a").unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &session_id,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            // Store stderr artifact for failed execution
            let cas_dir = omen_knowledge::resolve_workspace_dir(ws_path).join("cas");
            let cas = ContentAddressedStore::new(cas_dir);
            let stderr_bytes =
                b"error[E0432]: unresolved import `foo::bar`\n   --> src/main.rs:2:5\ntest failed";
            let stderr_meta = cas
                .store(
                    &mut db,
                    stderr_bytes,
                    "text/plain",
                    "human",
                    omen_core::RetentionClass::Referenced,
                )
                .unwrap();

            // Record failed cargo test execution
            let exec_fail = ExecutionRecord {
                execution_id: ExecutionId::new("exec-fail-1").unwrap(),
                session_id: session_id.clone(),
                command: "cargo test".into(),
                exit_code: Some(101),
                duration_ms: Some(340),
                stdout_artifact: None,
                stderr_artifact: Some(stderr_meta.uri.to_string()),
                envelope_json: None,
                created_at: "2026-09-19T04:00:00Z".into(),
            };
            ExecutionHistory::record_execution(&mut db, &exec_fail, &[], &[], &[]).unwrap();

            // Record a dirty fact: set dep gen=1, publish fact with gen=1, then bump dep gen=2
            FactRegistry::set_generation(&mut db, "fs:workspace", 1).unwrap();
            let fact_uri = ResourceUri::parse("fact://project/build_status").unwrap();
            FactRegistry::publish_fact(
                &mut db,
                PublishFactRequest {
                    resource: &fact_uri,
                    value: "stale_build",
                    assurance: Assurance::Observed,
                    producer: "test_harness",
                    witness: None,
                    dependencies: &[("fs:workspace".into(), 1)],
                    artifacts: &[],
                },
            )
            .unwrap();
            FactRegistry::increment_generation(&mut db, "fs:workspace").unwrap();

            ctx.phase("DISPATCH_IM_LOST");
            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                Some(db),
                None,
            )
            .unwrap();

            let exit = session.dispatch_input("? I'm lost").unwrap();
            assert!(exit.is_zero(), "AI lane dispatch must return exit code 0");

            // Verify with direct agent dispatcher to inspect structured output
            let out = omen_interactive::ai_lane::AiLaneDispatcher::dispatch_with_session(
                "I'm lost",
                &session_id,
                ws_path,
                session.db.as_ref(),
                session.agent_provider.as_deref(),
                Some(&session.comp_ctx),
            )
            .unwrap();

            assert!(out.configured, "Agent provider must be configured");
            assert!(
                out.response_text.contains("Agent"),
                "Must identify as Agent"
            );
            assert!(
                out.response_text.contains("cargo test") || out.response_text.contains("failed"),
                "Response should mention recent failure"
            );
            assert!(
                out.response_text.contains("dirty") || out.response_text.contains("build_status"),
                "Response should mention dirty facts"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_lane_why_did_that_fail_proof_b() {
    run_with_test_timeout(
        "test_agent_lane_why_did_that_fail_proof_b",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_WORKSPACE");
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let session_id = InteractiveSessionId::new("sess-proof-b").unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &session_id,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            // Store stderr artifact for failed cargo build
            let cas_dir = omen_knowledge::resolve_workspace_dir(ws_path).join("cas");
            let cas = ContentAddressedStore::new(cas_dir);
            let stderr_bytes = b"error[E0308]: mismatched types: expected `u32`, found `&str`";
            let stderr_meta = cas
                .store(
                    &mut db,
                    stderr_bytes,
                    "text/plain",
                    "human",
                    omen_core::RetentionClass::Referenced,
                )
                .unwrap();

            let exec_fail = ExecutionRecord {
                execution_id: ExecutionId::new("exec-fail-2").unwrap(),
                session_id: session_id.clone(),
                command: "cargo build --all-targets".into(),
                exit_code: Some(101),
                duration_ms: Some(120),
                stdout_artifact: None,
                stderr_artifact: Some(stderr_meta.uri.to_string()),
                envelope_json: None,
                created_at: "2026-09-19T04:10:00Z".into(),
            };
            ExecutionHistory::record_execution(&mut db, &exec_fail, &[], &[], &[]).unwrap();

            ctx.phase("DISPATCH_WHY_DID_THAT_FAIL");
            let session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                Some(db),
                None,
            )
            .unwrap();

            let out = omen_interactive::ai_lane::AiLaneDispatcher::dispatch_with_session(
                "why did that fail?",
                &session_id,
                ws_path,
                session.db.as_ref(),
                session.agent_provider.as_deref(),
                Some(&session.comp_ctx),
            )
            .unwrap();

            assert!(out.configured);
            assert!(out.response_text.contains("Agent"));
            assert!(out.response_text.contains("cargo build"));
            assert!(out.response_text.contains("mismatched types"));
            assert!(
                out.suggested_commands
                    .iter()
                    .any(|c| c.contains(":why") || c.contains(":show"))
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_lane_human_agent_build_proposal_proof_c() {
    run_with_test_timeout(
        "test_agent_lane_human_agent_build_proposal_proof_c",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("START_DAEMON");
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            fs::write(
                ws_path.join("Cargo.toml"),
                "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
            fs::create_dir_all(ws_path.join("src")).unwrap();
            fs::write(ws_path.join("src").join("lib.rs"), "pub fn sample() {}\n").unwrap();

            let daemon = Arc::new(DaemonServer::new(Some("duplex://proof_c_daemon".into())));
            let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);
            let instance_id = daemon.instance_id().to_string();
            let registry = daemon.registry();
            let shutdown_rx = daemon.subscribe_shutdown();

            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(
                    daemon_stream,
                    instance_id,
                    registry,
                    shutdown_rx,
                )
                .await;
            });

            let client = omen_client::OmenClient::from_stream(
                client_stream,
                Some("memory://proof_c".into()),
                Some("sess-proof-c".into()),
            )
            .await
            .unwrap();

            client
                .attach_workspace(ws_path.to_str().unwrap())
                .await
                .unwrap();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let session_id = InteractiveSessionId::new("sess-proof-c").unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &session_id,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                Some(db),
                Some(client),
            )
            .unwrap();

            ctx.phase("DISPATCH_BUILD_QUERY_AND_EXECUTE");
            let exit = session
                .dispatch_input("? check whether this project still builds")
                .unwrap();
            assert!(exit.is_zero(), "Permitted action must execute and succeed");

            let db_ref = session.db.as_ref().unwrap();
            let history =
                ExecutionHistory::list_session_executions(db_ref, &session_id, 10).unwrap();
            assert_eq!(history.len(), 1, "Exactly one execution recorded");
            assert_eq!(history[0].command, "cargo check");
            assert_eq!(history[0].exit_code, Some(0));

            let last = ReferenceResolver::resolve("@last", &session_id, db_ref).unwrap();
            assert_eq!(last, "cargo check", "@last must resolve to executed action");
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn real_human_agent_action_executes_through_daemon_once() {
    run_with_test_timeout(
        "real_human_agent_action_executes_through_daemon_once",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON_WORKSPACE");
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            fs::write(
                ws_path.join("Cargo.toml"),
                "[package]\nname = \"sample_once\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
            fs::create_dir_all(ws_path.join("src")).unwrap();
            fs::write(ws_path.join("src").join("lib.rs"), "pub fn check() {}\n").unwrap();

            let daemon = Arc::new(DaemonServer::new(Some("duplex://real_human_agent".into())));
            let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);
            let instance_id = daemon.instance_id().to_string();
            let registry = daemon.registry();
            let shutdown_rx = daemon.subscribe_shutdown();

            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(
                    daemon_stream,
                    instance_id,
                    registry,
                    shutdown_rx,
                )
                .await;
            });

            let client = omen_client::OmenClient::from_stream(
                client_stream,
                Some("memory://real_human_agent".into()),
                Some("sess-human-real".into()),
            )
            .await
            .unwrap();

            client
                .attach_workspace(ws_path.to_str().unwrap())
                .await
                .unwrap();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let session_id = InteractiveSessionId::new("sess-human-real").unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &session_id,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                Some(db),
                Some(client),
            )
            .unwrap();

            ctx.phase("DISPATCH_PERMITTED_ACTION");
            let exit = session
                .dispatch_input("? check whether this project still builds")
                .unwrap();
            assert!(exit.is_zero());

            // Check SQLite history has exactly 1 execution
            let db_ref = session.db.as_ref().unwrap();
            let history =
                ExecutionHistory::list_session_executions(db_ref, &session_id, 10).unwrap();
            assert_eq!(history.len(), 1, "Exactly one execution must occur");
            assert_eq!(history[0].command, "cargo check");
            assert_eq!(history[0].exit_code, Some(0));

            // Check session-scoped @last
            let last = ReferenceResolver::resolve("@last", &session_id, db_ref).unwrap();
            assert_eq!(last, "cargo check");
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_execution_id_matches_history_and_receipt() {
    run_with_test_timeout(
        "agent_execution_id_matches_history_and_receipt",
        INTEGRATION_TIMEOUT,
        |ctx| async move {
            ctx.phase("SETUP_DAEMON_WORKSPACE");
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            fs::write(
                ws_path.join("Cargo.toml"),
                "[package]\nname = \"sample_match\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
            fs::create_dir_all(ws_path.join("src")).unwrap();
            fs::write(ws_path.join("src").join("lib.rs"), "pub fn run() {}\n").unwrap();

            let daemon = Arc::new(DaemonServer::new(Some("duplex://match_receipt".into())));
            let (client_stream, daemon_stream) = PlatformStream::duplex_pair(65536);
            let instance_id = daemon.instance_id().to_string();
            let registry = daemon.registry();
            let shutdown_rx = daemon.subscribe_shutdown();

            tokio::spawn(async move {
                let _ = DaemonServer::handle_connection(
                    daemon_stream,
                    instance_id,
                    registry,
                    shutdown_rx,
                )
                .await;
            });

            let client = omen_client::OmenClient::from_stream(
                client_stream,
                Some("memory://match_receipt".into()),
                Some("sess-receipt-match".into()),
            )
            .await
            .unwrap();

            client
                .attach_workspace(ws_path.to_str().unwrap())
                .await
                .unwrap();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let session_id = InteractiveSessionId::new("sess-receipt-match").unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &session_id,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                Some(db),
                Some(client),
            )
            .unwrap();

            ctx.phase("EXECUTE_ACTION");
            let exit = session
                .dispatch_input("? check whether this project still builds")
                .unwrap();
            assert!(exit.is_zero());

            let db_ref = session.db.as_ref().unwrap();
            let history =
                ExecutionHistory::list_session_executions(db_ref, &session_id, 10).unwrap();
            assert_eq!(history.len(), 1);
            let exec_id_str = history[0].execution_id.as_str();

            // Verify receipt in SQLite
            let mut stmt = db_ref
                .conn()
                .prepare(
                    "SELECT execution_id, status FROM request_receipts WHERE execution_id = ?1",
                )
                .unwrap();
            let (receipt_exec_id, receipt_status): (String, String) = stmt
                .query_row([exec_id_str], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap();

            assert_eq!(
                receipt_exec_id, exec_id_str,
                "Execution ID must match between history and receipt"
            );
            assert_eq!(
                receipt_status, "Completed",
                "Receipt must report Completed status"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_background_work_preserves_human_last() {
    run_with_test_timeout(
        "agent_background_work_preserves_human_last",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let human_session = InteractiveSessionId::new("sess-human-bg").unwrap();
            let agent_session = InteractiveSessionId::new("sess-agent-bg").unwrap();

            ExecutionHistory::register_session(
                &mut db,
                &human_session,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &agent_session,
                "agent",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            // Human executes "cargo check"
            let human_exec = ExecutionRecord {
                execution_id: ExecutionId::new("exec-h-1").unwrap(),
                session_id: human_session.clone(),
                command: "cargo check".into(),
                exit_code: Some(0),
                duration_ms: Some(50),
                stdout_artifact: None,
                stderr_artifact: None,
                envelope_json: None,
                created_at: "2026-09-19T04:30:00Z".into(),
            };
            ExecutionHistory::record_execution(&mut db, &human_exec, &[], &[], &[]).unwrap();

            // Agent background worker executes "cargo build"
            let agent_exec = ExecutionRecord {
                execution_id: ExecutionId::new("exec-a-bg").unwrap(),
                session_id: agent_session.clone(),
                command: "cargo build".into(),
                exit_code: Some(0),
                duration_ms: Some(150),
                stdout_artifact: None,
                stderr_artifact: None,
                envelope_json: None,
                created_at: "2026-09-19T04:30:10Z".into(),
            };
            ExecutionHistory::record_execution(&mut db, &agent_exec, &[], &[], &[]).unwrap();

            // Verify human @last remains "cargo check"
            let human_last = ReferenceResolver::resolve("@last", &human_session, &db).unwrap();
            assert_eq!(human_last, "cargo check");

            // Agent @last resolves to its own command
            let agent_last = ReferenceResolver::resolve("@last", &agent_session, &db).unwrap();
            assert_eq!(agent_last, "cargo build");
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_context_preserves_workspace_root_after_cd() {
    run_with_test_timeout(
        "agent_context_preserves_workspace_root_after_cd",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp
                .path()
                .canonicalize()
                .unwrap_or_else(|_| temp.path().to_path_buf());
            let sub = ws_path.join("crates").join("test-sub");
            fs::create_dir_all(&sub).unwrap();

            let session_id = InteractiveSessionId::new("sess-cd-root").unwrap();
            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.clone(),
                None,
                None,
            )
            .unwrap();

            assert_eq!(session.workspace_root, ws_path);
            assert_eq!(session.cwd, ws_path);

            // Navigate into subdirectory
            let exit = session.dispatch_input("cd crates/test-sub").unwrap();
            assert!(exit.is_zero());

            let expected_sub = sub.canonicalize().unwrap_or(sub.clone());
            assert_eq!(session.cwd, expected_sub);
            assert_eq!(
                session.workspace_root, ws_path,
                "workspace_root must not change on cd"
            );

            // Verify agent context retains workspace_root
            let ctx = omen_interactive::ai_lane::build_agent_context_with_workspace(
                &session_id,
                &session.workspace_root,
                &session.cwd,
                None,
                None,
            );
            assert_eq!(ctx.workspace_root, ws_path);
            assert_eq!(ctx.cwd, session.cwd);
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_context_workspace_id_stable_after_cd() {
    run_with_test_timeout(
        "agent_context_workspace_id_stable_after_cd",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp
                .path()
                .canonicalize()
                .unwrap_or_else(|_| temp.path().to_path_buf());
            let sub = ws_path.join("nested");
            fs::create_dir_all(&sub).unwrap();

            let session_id = InteractiveSessionId::new("sess-ws-stable").unwrap();
            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.clone(),
                None,
                None,
            )
            .unwrap();

            let ctx_before = omen_interactive::ai_lane::build_agent_context_with_workspace(
                &session_id,
                &session.workspace_root,
                &session.cwd,
                None,
                None,
            );

            let exit = session.dispatch_input("cd nested").unwrap();
            assert!(exit.is_zero());

            let ctx_after = omen_interactive::ai_lane::build_agent_context_with_workspace(
                &session_id,
                &session.workspace_root,
                &session.cwd,
                None,
                None,
            );

            assert_eq!(
                ctx_before.workspace_id, ctx_after.workspace_id,
                "workspace_id must remain stable across cd"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_context_cas_uses_workspace_root() {
    run_with_test_timeout(
        "agent_context_cas_uses_workspace_root",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp
                .path()
                .canonicalize()
                .unwrap_or_else(|_| temp.path().to_path_buf());
            let sub = ws_path.join("subproject");
            fs::create_dir_all(&sub).unwrap();

            let db_path = canonical_workspace_db_path(&ws_path);
            let db = Database::open(&db_path).unwrap();

            let session_id = InteractiveSessionId::new("sess-cas-root").unwrap();
            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.clone(),
                Some(db),
                None,
            )
            .unwrap();

            // cd into subproject
            session.dispatch_input("cd subproject").unwrap();
            assert_ne!(session.cwd, session.workspace_root);

            // Store artifact via CAS using session.workspace_root
            let cas_dir =
                omen_knowledge::resolve_workspace_dir(&session.workspace_root).join("cas");
            let cas = ContentAddressedStore::new(cas_dir.clone());
            let meta = cas
                .store(
                    session.db.as_mut().unwrap(),
                    b"sample cas content",
                    "text/plain",
                    "test",
                    omen_core::RetentionClass::Referenced,
                )
                .unwrap();

            // Verify CAS directory uses workspace_root, not the nested cwd
            let cas_dir =
                omen_knowledge::resolve_workspace_dir(&session.workspace_root).join("cas");
            let expected_cas_dir = omen_knowledge::resolve_workspace_dir(&ws_path).join("cas");
            let sub_cas_dir = omen_knowledge::resolve_workspace_dir(&session.cwd).join("cas");

            assert_eq!(
                cas_dir, expected_cas_dir,
                "CAS directory must derive from workspace_root, not nested cwd"
            );
            assert_ne!(
                cas_dir, sub_cas_dir,
                "CAS directory must remain distinct from a subproject cwd-derived store"
            );
            assert!(meta.uri.as_str().starts_with("artifact://sha256/"));
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_lane_session_isolation_at_last() {
    run_with_test_timeout(
        "test_agent_lane_session_isolation_at_last",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            let db_path = canonical_workspace_db_path(ws_path);
            let mut db = Database::open(&db_path).unwrap();

            let human_session = InteractiveSessionId::new("sess-human-isolated").unwrap();
            let agent_session = InteractiveSessionId::new("sess-agent-external").unwrap();

            ExecutionHistory::register_session(
                &mut db,
                &human_session,
                "human",
                ws_path.to_str().unwrap(),
            )
            .unwrap();
            ExecutionHistory::register_session(
                &mut db,
                &agent_session,
                "agent",
                ws_path.to_str().unwrap(),
            )
            .unwrap();

            // Human executes "git status"
            let human_exec = ExecutionRecord {
                execution_id: ExecutionId::new("exec-human-1").unwrap(),
                session_id: human_session.clone(),
                command: "git status".into(),
                exit_code: Some(0),
                duration_ms: Some(15),
                stdout_artifact: None,
                stderr_artifact: None,
                envelope_json: None,
                created_at: "2026-09-19T04:20:00Z".into(),
            };
            ExecutionHistory::record_execution(&mut db, &human_exec, &[], &[], &[]).unwrap();

            // Background agent executes "npm run build" under its own session
            let agent_exec = ExecutionRecord {
                execution_id: ExecutionId::new("exec-agent-bg-1").unwrap(),
                session_id: agent_session.clone(),
                command: "npm run build".into(),
                exit_code: Some(0),
                duration_ms: Some(1200),
                stdout_artifact: None,
                stderr_artifact: None,
                envelope_json: None,
                created_at: "2026-09-19T04:20:05Z".into(),
            };
            ExecutionHistory::record_execution(&mut db, &agent_exec, &[], &[], &[]).unwrap();

            // Verify human @last resolves strictly to human command
            let resolved = ReferenceResolver::resolve("@last", &human_session, &db).unwrap();
            assert_eq!(resolved, "git status");

            // Run an interactive agent query in the human session
            let mut session = InteractiveSession::new_with_client(
                human_session.clone(),
                ws_path.to_path_buf(),
                Some(db),
                None,
            )
            .unwrap();

            let exit = session.dispatch_input("? what folder am I in?").unwrap();
            assert!(exit.is_zero());

            // Check that @last remains "git status"
            let resolved_after =
                ReferenceResolver::resolve("@last", &human_session, session.db.as_ref().unwrap())
                    .unwrap();
            assert_eq!(
                resolved_after, "git status",
                "Agent reasoning or background work must never mutate human session @last"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_lane_ambiguous_directory_navigation() {
    run_with_test_timeout(
        "test_agent_lane_ambiguous_directory_navigation",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            // Create two subdirectories with the same base name "components"
            fs::create_dir_all(ws_path.join("src").join("components")).unwrap();
            fs::create_dir_all(ws_path.join("tests").join("components")).unwrap();

            let session_id = InteractiveSessionId::new("sess-ambig").unwrap();
            let session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                None,
                None,
            )
            .unwrap();

            let out = omen_interactive::ai_lane::AiLaneDispatcher::dispatch_with_session(
                "switch to components",
                &session_id,
                ws_path,
                None,
                session.agent_provider.as_deref(),
                Some(&session.comp_ctx),
            )
            .unwrap();

            assert!(out.configured);
            assert!(
                out.response_text.contains("Found multiple")
                    || out.response_text.contains("Which one"),
                "Agent must refuse to silently pick an ambiguous directory"
            );
            assert!(
                out.response_text.contains("src/components")
                    || out.response_text.contains("src\\components")
            );
            assert!(
                out.response_text.contains("tests/components")
                    || out.response_text.contains("tests\\components")
            );
            assert!(
                out.proposed_actions.is_empty(),
                "Must not propose arbitrary action on ambiguity"
            );
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_lane_unambiguous_directory_navigation_executes() {
    run_with_test_timeout(
        "test_agent_lane_unambiguous_directory_navigation_executes",
        UNIT_TIMEOUT,
        |_ctx| async move {
            let temp = tempdir().unwrap();
            let ws_path = temp.path();

            // Create single unambiguous directory
            let target_sub = ws_path.join("crates").join("omen-engine");
            fs::create_dir_all(&target_sub).unwrap();

            let session_id = InteractiveSessionId::new("sess-nav").unwrap();
            let mut session = InteractiveSession::new_with_client(
                session_id.clone(),
                ws_path.to_path_buf(),
                None,
                None,
            )
            .unwrap();

            let initial_cwd = session.cwd.clone();
            let exit = session.dispatch_input("? switch to omen-engine").unwrap();
            assert!(exit.is_zero());

            assert_ne!(session.cwd, initial_cwd);
            assert!(session.cwd.ends_with("omen-engine"));
        },
    )
    .await;
}
