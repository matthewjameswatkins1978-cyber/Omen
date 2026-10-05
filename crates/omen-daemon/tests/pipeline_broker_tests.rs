use std::path::PathBuf;
use std::sync::Arc;

use omen_daemon::workspace::{BrokerPipelineParams, WorkspaceState};
use omen_ipc::PipelineStageRequest;
use tempfile::tempdir;

fn gremlin_exe() -> PathBuf {
    let mut executable = std::env::current_exe().expect("current test executable");
    executable.pop();
    if executable.ends_with("deps") {
        executable.pop();
    }
    let name = if cfg!(windows) {
        "omen-gremlin.exe"
    } else {
        "omen-gremlin"
    };
    let candidate = executable.join(name);
    if candidate.exists() {
        return candidate;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("daemon crate parent")
        .parent()
        .expect("workspace root")
        .join("target")
        .join("debug")
        .join(name)
}

#[tokio::test]
async fn pipeline_broker_keeps_receipt_history_and_stage_exit_truth() {
    let workspace = tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    let ws = Arc::new(WorkspaceState::new(root.clone(), 1).unwrap());
    let gremlin = gremlin_exe().to_string_lossy().to_string();
    let stages = vec![
        PipelineStageRequest {
            argv: vec![gremlin.clone(), "--stdout".into(), "brokered-pipe".into()],
            env: Vec::new(),
        },
        PipelineStageRequest {
            argv: vec![gremlin, "--echo-stdin".into()],
            env: Vec::new(),
        },
    ];
    let params = BrokerPipelineParams {
        dedup_id: "pipeline-request-1",
        session_id: "session-pipeline-1",
        stages: &stages,
        cwd: root.to_str().unwrap(),
        timeout_ms: 10000,
    };

    let summary = ws.execute_pipeline_broker(params.clone()).await.unwrap();
    assert_eq!(summary.runtime_status, omen_core::RuntimeStatus::Completed);
    assert_eq!(summary.exit_code, Some(0));
    assert_eq!(
        summary.pipeline_stage_exits,
        Some(vec![
            omen_core::ProcessExit {
                code: Some(0),
                signal: None,
            },
            omen_core::ProcessExit {
                code: Some(0),
                signal: None,
            },
        ])
    );
    assert!(summary.stdout_preview.contains("STDIN_ECHO:brokered-pipe"));
    assert!(summary.stdout_artifact.is_some());

    let repeated = ws.execute_pipeline_broker(params).await.unwrap();
    assert_eq!(repeated.execution_id, summary.execution_id);
    assert_eq!(repeated.pipeline_stage_exits, summary.pipeline_stage_exits);
}
