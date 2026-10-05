#![cfg(windows)]

use omen_core::PtySessionId;
use omen_engine::{NativePtyHandle, PtyExecutionHandle, PtyExecutionRequest};
use std::process::Stdio;
use std::time::Duration;

const DRIVER_ENV: &str = "OMEN_PTY_TERMINATION_DRIVER";
const FIXTURE_ENV: &str = "OMEN_PTY_TERMINATION_FIXTURE";

#[test]
fn windows_pty_termination_driver() {
    if std::env::var_os(DRIVER_ENV).is_none() {
        return;
    }

    let executable = std::env::current_exe().expect("test executable path");
    let request = PtyExecutionRequest {
        session_id: PtySessionId::generate(),
        argv: vec![
            executable.to_string_lossy().into_owned(),
            "--exact".into(),
            "windows_pty_termination_fixture".into(),
            "--nocapture".into(),
        ],
        cwd: std::env::current_dir().expect("current directory"),
        env: vec![(FIXTURE_ENV.into(), "1".into())],
        rows: 24,
        cols: 80,
    };

    let mut handle = NativePtyHandle::spawn(&request).expect("spawn ConPTY fixture");
    handle.terminate().expect("terminate ConPTY fixture");
    drop(handle);
}

#[test]
fn windows_pty_termination_fixture() {
    if std::env::var_os(FIXTURE_ENV).is_some() {
        std::thread::sleep(Duration::from_secs(300));
    }
}

#[test]
fn windows_pty_termination_closes_handles_once_in_child_process() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("Tokio runtime");

    runtime.block_on(async {
        let executable = std::env::current_exe().expect("test executable path");
        let mut child = tokio::process::Command::new(executable)
            .args(["--exact", "windows_pty_termination_driver", "--nocapture"])
            .env(DRIVER_ENV, "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn isolated termination driver");

        match tokio::time::timeout(Duration::from_secs(20), child.wait()).await {
            Ok(Ok(status)) => assert!(status.success(), "termination driver exited with {status}"),
            Ok(Err(error)) => panic!("wait for termination driver: {error}"),
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                panic!("termination driver exceeded its 20-second deadline");
            }
        }
    });
}
