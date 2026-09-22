//! E2 durable-history regression proofs: after the daemon commits an
//! execution, the CLI/MCP read-only history path MUST observe it while the
//! daemon is still alive (shutdown is never required to reveal work).
//
// ENV_LOCK discipline: the guard serializes OMEN_STATE_HOME mutation and
// is held for the whole scenario that depends on it, i.e. across awaits,
// by design. No other lock in this file is awaited while it is held and
// siblings only block on ENV_LOCK itself, so this cannot deadlock.
#![allow(clippy::await_holding_lock)]

use omen_client::OmenClient;
use omen_daemon::DaemonServer;
use omen_ipc::PlatformStream;
use omen_knowledge::db::Database;
use omen_knowledge::history::{HistoryQuery, query_history};
use omen_knowledge::workspace::canonical_workspace_db_path_readonly;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::tempdir;
use tokio::sync::watch;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn gremlin_exe() -> PathBuf {
    let mut path = std::env::current_exe().expect("failed to get current_exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let exe = path.join(name);
    if exe.exists() {
        return exe;
    }
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join(name);
    if fallback.exists() {
        return fallback;
    }
    let status = std::process::Command::new("cargo")
        .args(["build", "--bin", "omen-gremlin"])
        .status()
        .expect("failed to build omen-gremlin fixture");
    assert!(status.success(), "omen-gremlin build failed");
    fallback
}

#[tokio::test]
async fn brokered_execution_visible_to_readonly_history_while_daemon_alive() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");
    let state_home = tempdir().expect("state home tempdir");
    // ENV_LOCK is held for the whole scenario, so process-global env
    // mutation is sound here (serialized against sibling tests).
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        let server = DaemonServer::new(Some("e2-repro-endpoint".to_string()));
        let registry = server.registry();
        let instance_id = server.instance_id().to_string();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let tmp = tempdir().expect("workspace tempdir");
        let path = tmp.path().to_str().unwrap().to_string();
        let gremlin = gremlin_exe();

        let (c_stream, s_stream) = PlatformStream::duplex_pair(4096);
        let reg = registry.clone();
        let inst = instance_id.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            DaemonServer::handle_connection(s_stream, inst, reg, rx)
                .await
                .unwrap();
        });

        let client = OmenClient::from_stream(c_stream, None, Some("sess_repro".to_string()))
            .await
            .expect("phase=connect: client should connect");
        client
            .attach_workspace(&path)
            .await
            .expect("phase=attach: workspace attach");
        let summary = client
            .submit_consequential_execution(
                "req_e2_repro_visibility",
                "exec",
                gremlin.to_string_lossy().into_owned(),
                vec!["--stdout".into(), "e2-visible".into()],
                &path,
                10000,
            )
            .await
            .expect("phase=execute: brokered execution should succeed");
        assert_eq!(summary.exit_code, Some(0), "gremlin must exit 0");

        // Reader path identical to CLI `omen history` / MCP history query.
        let db_path = canonical_workspace_db_path_readonly(std::path::Path::new(&path));
        let reader = Database::open_read_only(&db_path)
            .expect("phase=history-open: canonical history database must exist");
        let history = query_history(
            &reader,
            &HistoryQuery {
                all_sessions: true,
                session_id: None,
                limit: 20,
            },
        )
        .expect("phase=history-query: history query must succeed");
        let found = history
            .entries
            .iter()
            .any(|e| e.execution_id.as_str() == summary.execution_id);
        assert!(
            found,
            "E2 read-after-write: brokered execution {} committed by a LIVE daemon must be visible to the read-only history path (got {} entries)",
            summary.execution_id,
            history.entries.len()
        );
    })
    .await;
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
    outcome.expect("phase=outer-watchdog: E2 live-visibility scenario timed out after 120s");
}

#[tokio::test]
async fn multiple_live_executions_retain_order_and_identity() {
    let _guard = ENV_LOCK.lock().expect("env lock poisoned");
    let state_home = tempdir().expect("state home tempdir");
    // ENV_LOCK is held for the whole scenario, so process-global env
    // mutation is sound here (serialized against sibling tests).
    unsafe {
        std::env::set_var("OMEN_STATE_HOME", state_home.path());
    }
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(180), async {
        let server = DaemonServer::new(Some("e2-order-endpoint".to_string()));
        let registry = server.registry();
        let instance_id = server.instance_id().to_string();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let tmp = tempdir().expect("workspace tempdir");
        let path = tmp.path().to_str().unwrap().to_string();
        let gremlin = gremlin_exe();

        let (c_stream, s_stream) = PlatformStream::duplex_pair(4096);
        let reg = registry.clone();
        let inst = instance_id.clone();
        let rx = shutdown_rx.clone();
        tokio::spawn(async move {
            DaemonServer::handle_connection(s_stream, inst, reg, rx)
                .await
                .unwrap();
        });

        let client = OmenClient::from_stream(c_stream, None, Some("sess_order".to_string()))
            .await
            .expect("phase=connect: client should connect");
        client
            .attach_workspace(&path)
            .await
            .expect("phase=attach: workspace attach");

        let mut submitted: Vec<String> = Vec::new();
        for i in 0..3 {
            let summary = client
                .submit_consequential_execution(
                    format!("req_e2_order_{i}"),
                    "exec",
                    gremlin.to_string_lossy().into_owned(),
                    vec!["--stdout".into(), format!("order-{i}")],
                    &path,
                    10000,
                )
                .await
                .unwrap_or_else(|_| panic!("phase=execute[{i}]: brokered execution should succeed"));
            assert_eq!(summary.exit_code, Some(0));
            submitted.push(summary.execution_id.clone());
        }

        let db_path = canonical_workspace_db_path_readonly(std::path::Path::new(&path));
        let reader = Database::open_read_only(&db_path)
            .expect("phase=history-open: canonical history database must exist");
        let history = query_history(
            &reader,
            &HistoryQuery {
                all_sessions: true,
                session_id: None,
                limit: 20,
            },
        )
        .expect("phase=history-query: history query must succeed");
        // Newest-first ordering: reverse of submission.
        let got: Vec<&str> = history
            .entries
            .iter()
            .map(|e| e.execution_id.as_str())
            .collect();
        for id in &submitted {
            assert!(
                got.contains(&id.as_str()),
                "E2 order/identity: submitted execution {id} must be visible live (got {got:?})"
            );
        }
        let positions: Vec<usize> = submitted
            .iter()
            .map(|id| got.iter().position(|g| g == id).unwrap())
            .collect();
        assert!(
            positions[0] > positions[1] && positions[1] > positions[2],
            "E2 order/identity: newest-first order must hold live (positions {positions:?} for {submitted:?})"
        );
    })
    .await;
    unsafe {
        std::env::remove_var("OMEN_STATE_HOME");
    }
    outcome.expect("phase=outer-watchdog: E2 order scenario timed out after 180s");
}
