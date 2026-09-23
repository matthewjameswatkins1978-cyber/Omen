//! Out-of-process adapter child supervisor.
//!
//! argv-only launch, explicit environment (never inherited), NDJSON
//! framing with byte bounds, explicit timeouts, cancellation via Drop,
//! graceful shutdown with bounded grace, tree-kill on Windows, truthful
//! exit reporting. No unbounded waits anywhere.

use omen_agent_adapter::protocol::{
    AdapterFrame, MAX_FRAME_BYTES, OmenFrame, ProtocolError, decode_adapter_frame, encode_frame,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

/// Maximum stderr bytes retained (tail) for diagnostics.
pub const MAX_STDERR_TAIL_BYTES: usize = 8192;
/// Grace period for cooperative shutdown before kill.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct SpawnLimits {
    pub frame_timeout: Duration,
    pub total_timeout: Duration,
    pub shutdown_grace: Duration,
}

impl Default for SpawnLimits {
    fn default() -> Self {
        Self {
            frame_timeout: Duration::from_secs(30),
            total_timeout: Duration::from_secs(120),
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitTruth {
    pub code: Option<i32>,
    pub killed_by_omen: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterSpawnError {
    #[error("adapter spawn failed for {exe:?}: {message}")]
    SpawnFailed { exe: PathBuf, message: String },
    #[error("adapter IO failure: {0}")]
    Io(String),
    #[error("adapter protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("adapter frame wait exceeded {0:?}")]
    FrameTimeout(Duration),
    #[error("adapter closed stdout (EOF) with no further frames")]
    Eof,
    #[error("adapter exited with status {code:?} (stderr tail: {stderr_tail})")]
    Exited {
        code: Option<i32>,
        stderr_tail: String,
    },
}

pub struct SpawnedAdapter {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    pid: u32,
    limits: SpawnLimits,
    killed: bool,
}

impl Drop for SpawnedAdapter {
    fn drop(&mut self) {
        // Cancellation path: dropping the exchange future must not leave an
        // endless child behind. Best-effort synchronous kill; the OS reaps.
        let _ = self.child.start_kill();
        if let Some(task) = &self.stderr_task {
            task.abort();
        }
    }
}

impl SpawnedAdapter {
    /// Launches an adapter child. argv-only; environment is exactly `env`
    /// (the parent environment is never inherited). stdin/stdout/stderr piped.
    pub async fn launch(
        exe: &Path,
        argv: &[String],
        env: &[(String, String)],
        cwd: &Path,
        limits: SpawnLimits,
    ) -> Result<Self, AdapterSpawnError> {
        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(argv);
        cmd.current_dir(cwd);
        cmd.env_clear();
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // Detached from the Omen console; protocol stdout stays machine-clean.
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000);
        }
        let mut child = cmd.spawn().map_err(|e| AdapterSpawnError::SpawnFailed {
            exe: exe.to_path_buf(),
            message: e.to_string(),
        })?;
        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AdapterSpawnError::Io("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AdapterSpawnError::Io("no stderr".into()))?;
        let tail: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let tail_clone = Arc::clone(&tail);
        let stderr_task = Some(tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = vec![0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut guard = tail_clone.lock().unwrap();
                        guard.extend_from_slice(&buf[..n]);
                        if guard.len() > MAX_STDERR_TAIL_BYTES {
                            let drop_n = guard.len() - MAX_STDERR_TAIL_BYTES;
                            guard.drain(..drop_n);
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr_tail: tail,
            stderr_task,
            pid,
            limits,
            killed: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn stderr_tail_text(&self) -> String {
        let guard = self.stderr_tail.lock().unwrap();
        String::from_utf8_lossy(&guard).to_string()
    }

    /// Sends one frame (newline-terminated JSON) to the child stdin.
    pub async fn send(&mut self, frame: &OmenFrame) -> Result<(), AdapterSpawnError> {
        let line = encode_frame(frame)?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| AdapterSpawnError::Io("stdin closed".into()))?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AdapterSpawnError::Io(e.to_string()))?;
        stdin
            .flush()
            .await
            .map_err(|e| AdapterSpawnError::Io(e.to_string()))?;
        Ok(())
    }

    /// Reads the next adapter frame with an explicit ceiling. Never blocks
    /// forever; oversized lines fail without unbounded buffering.
    pub async fn next_frame(&mut self) -> Result<AdapterFrame, AdapterSpawnError> {
        let timeout = self.limits.frame_timeout;
        match tokio::time::timeout(timeout, self.read_line_bounded()).await {
            Ok(result) => result,
            Err(_) => Err(AdapterSpawnError::FrameTimeout(timeout)),
        }
    }

    async fn read_line_bounded(&mut self) -> Result<AdapterFrame, AdapterSpawnError> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            match self.stdout.read_exact(&mut byte).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    if buf.is_empty() {
                        return Err(AdapterSpawnError::Eof);
                    }
                    return Err(AdapterSpawnError::Protocol(ProtocolError::Malformed(
                        "truncated final frame (EOF before newline)".into(),
                    )));
                }
                Err(e) => return Err(AdapterSpawnError::Io(e.to_string())),
            }
            if byte[0] == b'\n' {
                break;
            }
            buf.push(byte[0]);
            if buf.len() > MAX_FRAME_BYTES + 1 {
                return Err(AdapterSpawnError::Protocol(ProtocolError::FrameTooLarge {
                    bytes: buf.len(),
                    max: MAX_FRAME_BYTES,
                }));
            }
        }
        // Tolerate CRLF from Windows-native children.
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        let line = String::from_utf8(buf).map_err(|_| {
            AdapterSpawnError::Protocol(ProtocolError::Malformed("frame is not valid UTF-8".into()))
        })?;
        if line.trim().is_empty() {
            return Err(AdapterSpawnError::Protocol(ProtocolError::Malformed(
                "empty frame".into(),
            )));
        }
        Ok(decode_adapter_frame(&line)?)
    }

    /// Graceful shutdown: close stdin, wait bounded grace, then kill
    /// (process tree on Windows) and reap. Always returns the truth.
    pub async fn shutdown(mut self) -> ExitTruth {
        // Close stdin first so well-behaved adapters exit on their own.
        drop(self.stdin.take());
        if let Some(task) = self.stderr_task.take() {
            let _ = tokio::time::timeout(Duration::from_millis(200), task).await;
        }
        match tokio::time::timeout(self.limits.shutdown_grace, self.child.wait()).await {
            Ok(Ok(status)) => ExitTruth {
                code: status.code(),
                killed_by_omen: self.killed,
            },
            _ => {
                self.kill_tree().await;
                match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
                    Ok(Ok(status)) => ExitTruth {
                        code: status.code(),
                        killed_by_omen: true,
                    },
                    _ => ExitTruth {
                        code: None,
                        killed_by_omen: true,
                    },
                }
            }
        }
    }

    /// Kills the child now (and its tree on Windows), then reaps.
    pub async fn kill(mut self) -> ExitTruth {
        self.kill_tree().await;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
        ExitTruth {
            code: None,
            killed_by_omen: true,
        }
    }

    async fn kill_tree(&mut self) {
        self.killed = true;
        #[cfg(windows)]
        {
            // TerminateProcess alone orphans sandboxed grandchildren; the
            // bounded taskkill reaps the whole tree. Failure is non-fatal:
            // start_kill below still targets the direct child.
            let pid = self.pid;
            if pid != 0 {
                let mut cmd = tokio::process::Command::new("taskkill");
                cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
                cmd.stdout(Stdio::null());
                cmd.stderr(Stdio::null());
                #[cfg(windows)]
                {
                    cmd.creation_flags(0x08000000);
                }
                if let Ok(mut killer) = cmd.spawn() {
                    let _ = tokio::time::timeout(Duration::from_secs(5), killer.wait()).await;
                }
            }
        }
        let _ = self.child.start_kill();
    }
}
