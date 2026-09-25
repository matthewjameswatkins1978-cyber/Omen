//! Managed Tethers Gate companion process over persistent local stdio.
//!
//! The Gate is a **managed companion, not an interactive shell command**:
//! one physical owner per session, explicit startup/request timeouts,
//! bounded frames, strict NDJSON, bounded stderr capture, allowlisted
//! environment, explicit restart policy (there is none automatic — a
//! faulted session is dead; the caller starts a FRESH session and never
//! reuses stale authority).
//!
//! First transport fault kills the session fail-closed: a Gate that
//! cannot answer in time, answers malformed frames, or dies is
//! unavailable, and authority-required work refuses. `Drop` kills the
//! child; orderly end uses [`GateProcess::shutdown`].

use crate::AuthorityError;
use crate::protocol::{AUTHORITY_PROTOCOL, HelloResult, MAX_FRAME_BYTES, RequestFrame};
use crate::transport::{GateTransport, RoundtripError};
use serde_json::Value;
use std::io::Read;
use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// How to spawn the Gate companion.
#[derive(Debug, Clone)]
pub struct GateSpawnConfig {
    /// Absolute path to the Gate binary (hash-verified before spawn when
    /// `expected_exe_sha256` is set).
    pub exe: std::path::PathBuf,
    /// Full argv after the binary (gate flags; absolute paths).
    pub args: Vec<String>,
    /// Pinned executable identity. `None` is allowed only for explicitly
    /// unmanaged/test transports — managed production spawns always pin.
    pub expected_exe_sha256: Option<String>,
    /// Explicit extra environment for the child (after `env_clear` +
    /// platform minimum). Ambient secrets are NEVER inherited.
    pub env_extra: Vec<(String, String)>,
    /// Bound for process start + hello handshake.
    pub startup_timeout: Duration,
    /// Bound for any single request/response exchange.
    pub request_timeout: Duration,
    /// Cap for captured stderr (bytes kept, tail).
    pub stderr_cap: usize,
}

impl GateSpawnConfig {
    /// Platform-minimum environment a native child needs to start.
    /// Deliberately tiny: PATH (engine/tool resolution by absolute path
    /// still works, but subprocess lookup does not depend on ambient
    /// state), SYSTEMROOT + TEMP/TMP on Windows. Everything else must be
    /// passed explicitly via `env_extra`.
    pub fn minimal_env() -> Vec<(String, String)> {
        let mut env = Vec::new();
        for key in [
            "PATH",
            "TEMP",
            "TMP",
            "SystemRoot",
            "SYSTEMROOT",
            "HOME",
            "LANG",
            "LC_ALL",
        ] {
            if let Ok(v) = std::env::var(key) {
                env.push((key.to_string(), v));
            }
        }
        env
    }
}

/// Default timeouts: startup 20 s (engine-adjacent init), request 60 s
/// (Core planning happens inside PREPARE), shutdown grace 5 s.
pub const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const DEFAULT_STDERR_CAP: usize = 64 * 1024;

enum ReaderMsg {
    Line(String),
    Oversized,
    Eof,
    IoError(String),
}

/// One supervised Gate session. Not `Clone`: one physical owner.
#[derive(Debug)]
pub struct GateProcess {
    label: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    rx: Receiver<ReaderMsg>,
    stderr_tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    request_timeout: Duration,
    next_id: AtomicU64,
    dead: bool,
}

impl GateProcess {
    /// Verify identity, spawn with an allowlisted environment, start the
    /// reader threads. Does NOT handshake — call [`GateProcess::hello`].
    pub fn spawn(config: &GateSpawnConfig) -> Result<Self, AuthorityError> {
        if let Some(expected) = &config.expected_exe_sha256 {
            let actual = sha256_file(&config.exe)
                .map_err(|e| AuthorityError::Discover(format!("gate.hash.failed: {e}")))?;
            if &actual != expected {
                return Err(AuthorityError::Discover(format!(
                    "gate.identity_mismatch: executable hash {actual} != pinned {expected}"
                )));
            }
        }
        let mut cmd = Command::new(&config.exe);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        for (k, v) in GateSpawnConfig::minimal_env() {
            cmd.env(k, v);
        }
        for (k, v) in &config.env_extra {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| AuthorityError::Initialize(format!("gate.spawn.failed: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AuthorityError::Initialize("gate.spawn.failed: no stdin".to_string()))?;
        let stdout: ChildStdout = child.stdout.take().ok_or_else(|| {
            AuthorityError::Initialize("gate.spawn.failed: no stdout".to_string())
        })?;
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("omen-authority-gate-stdout".to_string())
            .spawn(move || read_lines_bounded(stdout, tx))
            .map_err(|e| AuthorityError::Initialize(format!("gate.supervision.failed: {e}")))?;
        let stderr_tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        if let Some(err) = stderr {
            let tail = std::sync::Arc::clone(&stderr_tail);
            let cap = config.stderr_cap.max(1024);
            std::thread::Builder::new()
                .name("omen-authority-gate-stderr".to_string())
                .spawn(move || drain_capped(err, tail, cap))
                .ok();
        }
        Ok(Self {
            label: format!("gate:{}", config.exe.display()),
            child: Some(child),
            stdin: Some(stdin),
            rx,
            stderr_tail,
            request_timeout: config.request_timeout,
            next_id: AtomicU64::new(1),
            dead: false,
        })
    }

    pub fn next_request_id(&self) -> String {
        format!("req-{}", self.next_id.fetch_add(1, Ordering::SeqCst))
    }

    /// `hello` handshake with the startup bound. Must be called first.
    pub fn hello(&mut self, startup_timeout: Duration) -> Result<HelloResult, AuthorityError> {
        let saved = self.request_timeout;
        self.request_timeout = startup_timeout;
        let out = self.roundtrip_checked("hello", Value::Object(Default::default()));
        self.request_timeout = saved;
        let (_, raw) = out.map_err(|e| match e {
            RoundtripError::Transport(err) => err,
            RoundtripError::Refused { code, message, .. } => {
                AuthorityError::Initialize(format!("gate.hello.refused: {code}: {message}"))
            }
        })?;
        serde_json::from_value(raw)
            .map_err(|e| AuthorityError::Initialize(format!("gate.hello.malformed: {e}")))
    }

    /// Response `request_id` echo check: responses that do not echo the
    /// request identity kill the session. (The Gate echoes `request_id`;
    /// `interpret` validates framing — the echo binding lives here.)
    pub fn roundtrip_checked(
        &mut self,
        operation: &str,
        payload: Value,
    ) -> Result<(String, Value), RoundtripError> {
        if self.dead {
            return Err(RoundtripError::Transport(AuthorityError::Supervision(
                "gate.session.dead".to_string(),
            )));
        }
        let request_id = self.next_request_id();
        let frame = RequestFrame::new(request_id.clone(), operation, payload);
        let line = frame.to_line().map_err(RoundtripError::Transport)?;
        if let Some(stdin) = self.stdin.as_mut() {
            if let Err(e) = stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()) {
                self.kill_now();
                return Err(RoundtripError::Transport(AuthorityError::Request(format!(
                    "gate.write.failed: {e}"
                ))));
            }
        } else {
            self.kill_now();
            return Err(RoundtripError::Transport(AuthorityError::Receive(
                "gate.stdin.closed".to_string(),
            )));
        }
        let raw_line = match self.rx.recv_timeout(self.request_timeout) {
            Ok(ReaderMsg::Line(line)) => line,
            Ok(ReaderMsg::Oversized) => {
                self.kill_now();
                return Err(RoundtripError::Transport(AuthorityError::Receive(
                    "frame.oversized from gate".to_string(),
                )));
            }
            Ok(ReaderMsg::Eof) => {
                self.kill_now();
                return Err(RoundtripError::Transport(AuthorityError::Receive(
                    "gate.eof: gate closed stdout".to_string(),
                )));
            }
            Ok(ReaderMsg::IoError(e)) => {
                self.kill_now();
                return Err(RoundtripError::Transport(AuthorityError::Receive(format!(
                    "gate.stdout.failed: {e}"
                ))));
            }
            Err(_) => {
                self.kill_now();
                return Err(RoundtripError::Transport(AuthorityError::Receive(format!(
                    "gate.response.timeout after {}s",
                    self.request_timeout.as_secs()
                ))));
            }
        };
        let parsed =
            crate::protocol::ResponseFrame::parse(&raw_line).map_err(RoundtripError::Transport)?;
        if parsed.request_id != request_id {
            self.kill_now();
            return Err(RoundtripError::Transport(AuthorityError::Receive(format!(
                "gate.request_id_mismatch: want {request_id} got {}",
                parsed.request_id
            ))));
        }
        if parsed.schema != AUTHORITY_PROTOCOL {
            self.kill_now();
            return Err(RoundtripError::Transport(AuthorityError::Receive(
                "frame.unsupported_schema from gate".to_string(),
            )));
        }
        parsed
            .into_result()
            .map(|v| (request_id, v))
            .map_err(|e| RoundtripError::Refused {
                code: e.code,
                message: e.message,
                data: e.data,
            })
    }

    /// Bounded stderr tail for diagnostics (never secrets: the Gate gets
    /// an allowlisted environment and protocol frames carry no secrets).
    pub fn stderr_tail(&self) -> Vec<u8> {
        self.stderr_tail
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    fn kill_now(&mut self) {
        self.dead = true;
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // Bounded reap: never block supervision on a dying child.
            std::thread::Builder::new()
                .name("omen-authority-gate-reap".to_string())
                .spawn(move || {
                    let _ = child.wait();
                })
                .ok();
        }
    }

    /// Orderly end: `shutdown` frame, bounded grace, then kill.
    /// Idempotent; safe to call twice.
    pub fn shutdown_graceful(&mut self, grace: Duration) {
        if self.dead {
            return;
        }
        let saved = self.request_timeout;
        self.request_timeout = grace;
        let _ = self.roundtrip_checked("shutdown", Value::Object(Default::default()));
        self.request_timeout = saved;
        self.kill_now();
    }
}

impl GateTransport for GateProcess {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn roundtrip(&mut self, operation: &str, payload: Value) -> Result<Value, RoundtripError> {
        self.roundtrip_checked(operation, payload).map(|(_, v)| v)
    }

    fn shutdown(&mut self) {
        self.shutdown_graceful(DEFAULT_SHUTDOWN_GRACE);
    }
}

impl Drop for GateProcess {
    fn drop(&mut self) {
        self.kill_now();
    }
}

fn sha256_file(path: &std::path::Path) -> Result<String, std::io::Error> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Bounded stdout line reader: one NDJSON frame per line, hard cap.
/// Oversized input poisons the session (fail closed, never unbounded).
fn read_lines_bounded(stdout: ChildStdout, tx: Sender<ReaderMsg>) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::with_capacity(8192);
    loop {
        buf.clear();
        // Manual byte loop with a hard cap: BufRead::read_until would
        // grow unbounded on a newline-less flood.
        let mut capped = (&mut reader).take((MAX_FRAME_BYTES + 1) as u64);
        let mut byte = [0u8; 1];
        let mut len = 0usize;
        let mut newline = false;
        loop {
            match capped.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    len += 1;
                    if byte[0] == b'\n' {
                        newline = true;
                        break;
                    }
                    buf.push(byte[0]);
                }
                Err(_) => {
                    let _ = tx.send(ReaderMsg::IoError("stdout read failed".to_string()));
                    return;
                }
            }
        }
        if len == 0 && !newline {
            let _ = tx.send(ReaderMsg::Eof);
            return;
        }
        if len > MAX_FRAME_BYTES {
            let _ = tx.send(ReaderMsg::Oversized);
            // Drain to the next newline so framing can never resync
            // onto attacker-chosen bytes — then EOF the session.
            let mut drain = [0u8; 4096];
            loop {
                match reader.read(&mut drain) {
                    Ok(0) => break,
                    Ok(n) => {
                        if drain[..n].contains(&b'\n') {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(ReaderMsg::Eof);
            return;
        }
        let mut line = String::from_utf8_lossy(&buf).into_owned();
        while line.ends_with('\r') || line.ends_with('\n') {
            line.pop();
        }
        if tx.send(ReaderMsg::Line(line)).is_err() {
            return;
        }
        if len == 0 {
            return;
        }
    }
}

fn drain_capped(
    stderr: std::process::ChildStderr,
    tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    cap: usize,
) {
    let mut reader = BufReader::new(stderr);
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Ok(mut guard) = tail.lock() {
                    guard.extend_from_slice(&chunk[..n]);
                    let overflow = guard.len().saturating_sub(cap);
                    if overflow > 0 {
                        guard.drain(..overflow);
                    }
                }
            }
            Err(_) => break,
        }
    }
}

pub fn default_spawn_config(
    exe: std::path::PathBuf,
    args: Vec<String>,
    expected_exe_sha256: Option<String>,
) -> GateSpawnConfig {
    GateSpawnConfig {
        exe,
        args,
        expected_exe_sha256,
        env_extra: Vec::new(),
        startup_timeout: DEFAULT_STARTUP_TIMEOUT,
        request_timeout: DEFAULT_REQUEST_TIMEOUT,
        stderr_cap: DEFAULT_STDERR_CAP,
    }
}
