use crate::core::{CommandSpec, EnvPolicy, EvidenceGrade, ExitCause, OutputBounds, StdinSpec};
use crate::invariant::{InvariantId, InvariantOutcome, InvariantResult};
use crate::observation::{Observation, StreamKind};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Default reaping bound after a timeout kill. Separate from the scenario
/// deadline so cleanup is bounded but still truthfully attempted.
const DEFAULT_REAP_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Bounded capture for one stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CapturedStream {
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
}

impl CapturedStream {
    pub fn as_lossy_string(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Truthful record of post-deadline cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CleanupOutcome {
    pub attempted: bool,
    pub kill_succeeded: bool,
    pub reaped: bool,
    pub detail: String,
}

/// Structured result of one bounded run. Always returned; never hangs past
/// the deadline plus cleanup bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub observations: Vec<Observation>,
    pub exit_cause: Option<ExitCause>,
    pub stdout: CapturedStream,
    pub stderr: CapturedStream,
    pub timed_out: bool,
    pub cleanup: CleanupOutcome,
    pub elapsed_ms: u64,
    pub deadline_ms: u64,
    pub bounds: OutputBounds,
    pub harness_failed: bool,
    pub pid: Option<u32>,
}

impl RunOutcome {
    pub fn stdout_lossy(&self) -> String {
        self.stdout.as_lossy_string()
    }

    pub fn stderr_lossy(&self) -> String {
        self.stderr.as_lossy_string()
    }

    /// Locate the last single-line JSON object on stdout (fixture reports).
    pub fn fixture_report(&self) -> Option<serde_json::Value> {
        let text = self.stdout_lossy();
        for line in text.lines().rev() {
            let line = line.trim();
            if line.starts_with('{')
                && line.ends_with('}')
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            {
                return Some(value);
            }
        }
        None
    }

    pub fn capture_summary(stream: &CapturedStream) -> crate::failure::StreamSummary {
        crate::failure::StreamSummary::from_capture(
            &stream.bytes,
            stream.total_bytes,
            stream.truncated,
        )
    }

    /// BOUNDED_WAIT_NO_HANG: the runner itself reached a terminal outcome
    /// within the scenario deadline (plus bounded cleanup when timed out).
    pub fn check_bounded_wait_no_hang(&self) -> InvariantResult {
        let invariant = InvariantId::BoundedWaitNoHang;
        if self.harness_failed {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                "harness reported failure before a terminal outcome",
            )
            .with_observations(self.observations.clone());
        }
        let reached_terminal = self.exit_cause.is_some() || self.timed_out;
        if reached_terminal {
            InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "runner returned terminal outcome in {}ms (deadline {}ms)",
                    self.elapsed_ms, self.deadline_ms
                ),
            )
            .with_observations(self.observations.clone())
        } else {
            InvariantResult::inconclusive(
                invariant,
                EvidenceGrade::Partial,
                "no exit cause and no timeout recorded",
            )
            .with_observations(self.observations.clone())
        }
    }

    /// EXIT_CAUSE_PRESERVED: ordinary portable exit code must remain exact.
    pub fn check_exit_cause_preserved(&self, expected_code: i32) -> InvariantResult {
        let invariant = InvariantId::ExitCausePreserved;
        if self.timed_out {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "expected exit code {expected_code} but run timed out with cause TimeoutKill"
                ),
            )
            .with_observations(self.observations.clone());
        }
        match &self.exit_cause {
            Some(ExitCause::Code { code }) if *code == expected_code => InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                format!("observed exact exit code {code}"),
            )
            .with_observations(self.observations.clone()),
            Some(ExitCause::Code { code }) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!("expected exit code {expected_code}, observed {code}"),
            )
            .with_observations(self.observations.clone()),
            Some(other) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "expected portable exit code {expected_code}, observed non-code cause {}",
                    other.stable_id()
                ),
            )
            .with_observations(self.observations.clone()),
            None => {
                InvariantResult::unavailable(invariant, "no exit cause recorded before judgment")
                    .with_observations(self.observations.clone())
            }
        }
    }

    /// DRAIN_BOTH_STREAMS_NO_DEADLOCK: both streams must be captured without
    /// hanging. Truncation is allowed and must remain explicit.
    pub fn check_drain_both_streams_no_deadlock(&self) -> InvariantResult {
        let invariant = InvariantId::DrainBothStreamsNoDeadlock;
        if self.harness_failed {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                "harness failed while draining streams",
            )
            .with_observations(self.observations.clone());
        }
        if self.timed_out {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                "run timed out; drain did not reach completion without deadlock",
            )
            .with_observations(self.observations.clone());
        }
        if self.exit_cause.is_none() {
            return InvariantResult::inconclusive(
                invariant,
                EvidenceGrade::Partial,
                "exit cause missing; cannot prove independent drain completed",
            )
            .with_observations(self.observations.clone());
        }
        InvariantResult::pass(
            invariant,
            EvidenceGrade::Strong,
            format!(
                "both streams drained; stdout_total={} stderr_total={} truncated_out={} truncated_err={}",
                self.stdout.total_bytes,
                self.stderr.total_bytes,
                self.stdout.truncated,
                self.stderr.truncated
            ),
        )
        .with_observations(self.observations.clone())
    }

    /// DESCRIPTOR_CLOSURE_EARLY_EXIT: child may exit while parent still holds
    /// write handles; the runner must still return.
    pub fn check_descriptor_closure_early_exit(&self) -> InvariantResult {
        let invariant = InvariantId::DescriptorClosureEarlyExit;
        if self.timed_out {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                "runner hung past deadline after early child exit (descriptor lifecycle)",
            )
            .with_observations(self.observations.clone());
        }
        match &self.exit_cause {
            Some(ExitCause::Code { .. }) => InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                "child exited while harness retained stream ownership; runner returned",
            )
            .with_observations(self.observations.clone()),
            Some(other) => InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "child terminal cause {} observed without harness hang",
                    other.stable_id()
                ),
            )
            .with_observations(self.observations.clone()),
            None => InvariantResult::unavailable(invariant, "no exit observed")
                .with_observations(self.observations.clone()),
        }
    }

    /// ZERO_UNSCRIPTED_INPUT_WRITES: under StdinSpec::Closed the fixture must
    /// observe EOF and zero bytes when its own report says so.
    pub fn check_zero_unscripted_input_writes(
        &self,
        stdin: &StdinSpec,
        fixture_bytes_read: Option<u64>,
        fixture_stdin_eof: Option<bool>,
    ) -> InvariantResult {
        let invariant = InvariantId::ZeroUnscriptedInputWrites;
        match stdin {
            StdinSpec::Bytes(expected) => {
                if self.timed_out {
                    return InvariantResult::inconclusive(
                        invariant,
                        EvidenceGrade::Partial,
                        "timed out before stdin report could be judged",
                    );
                }
                match fixture_bytes_read {
                    Some(n) if n == expected.len() as u64 => InvariantResult::pass(
                        invariant,
                        EvidenceGrade::Strong,
                        format!("fixture read exactly {n} scripted bytes"),
                    ),
                    Some(n) => InvariantResult::fail(
                        invariant,
                        EvidenceGrade::Strong,
                        format!(
                            "fixture read {n} bytes; scripted stdin was {} bytes",
                            expected.len()
                        ),
                    ),
                    None => InvariantResult::inconclusive(
                        invariant,
                        EvidenceGrade::Partial,
                        "no fixture stdin report observed",
                    ),
                }
            }
            StdinSpec::Inherited => InvariantResult::new(
                invariant,
                InvariantOutcome::Unsupported,
                EvidenceGrade::Unavailable,
                "inherited stdin is outside the M0 closed/bytes proof surface",
            ),
            StdinSpec::Closed => {
                if self.timed_out {
                    return InvariantResult::inconclusive(
                        invariant,
                        EvidenceGrade::Partial,
                        "timed out before stdin EOF report",
                    );
                }
                let eof_ok = fixture_stdin_eof.unwrap_or(false);
                let bytes = fixture_bytes_read;
                match (eof_ok, bytes) {
                    (true, Some(0)) => InvariantResult::pass(
                        invariant,
                        EvidenceGrade::Strong,
                        "fixture observed EOF with zero bytes; harness wrote nothing",
                    ),
                    (true, Some(n)) => InvariantResult::fail(
                        invariant,
                        EvidenceGrade::Strong,
                        format!("fixture reported {n} bytes under StdinSpec::Closed"),
                    )
                    .with_observations(self.observations.clone()),
                    (false, _) => InvariantResult::fail(
                        invariant,
                        EvidenceGrade::Strong,
                        "fixture did not observe stdin EOF under StdinSpec::Closed",
                    ),
                    (true, None) => InvariantResult::inconclusive(
                        invariant,
                        EvidenceGrade::Partial,
                        "EOF observed but fixture did not report byte count",
                    ),
                }
            }
        }
    }

    /// ENV_RECORDED: child environment policy and fixture-observed dedicated
    /// test value must match without dumping secrets.
    pub fn check_env_recorded(
        &self,
        expected_key: &str,
        expected_value: &str,
        fixture_value: Option<&str>,
    ) -> InvariantResult {
        let invariant = InvariantId::EnvRecorded;
        if self.timed_out {
            return InvariantResult::inconclusive(
                invariant,
                EvidenceGrade::Partial,
                "timed out before fixture env report",
            );
        }
        let applied = self.observations.iter().find_map(|obs| match obs {
            Observation::EnvRecorded { key, value } if key == expected_key => Some(value.clone()),
            _ => None,
        });

        match (applied, fixture_value) {
            (Some(applied_val), Some(fixture_val))
                if applied_val == expected_value && fixture_val == expected_value =>
            {
                InvariantResult::pass(
                    invariant,
                    EvidenceGrade::Strong,
                    format!("{expected_key} applied and observed as dedicated test value"),
                )
                .with_observations(self.observations.clone())
            }
            (Some(applied_val), Some(fixture_val)) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "env mismatch for {expected_key}: harness={applied_val} fixture={fixture_val} expected={expected_value}"
                ),
            ),
            (Some(_), None) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Partial,
                "harness recorded env application but fixture did not report the value",
            ),
            (None, _) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!("harness never recorded EnvRecorded for {expected_key}"),
            ),
        }
    }
}

fn apply_env(cmd: &mut Command, policy: &EnvPolicy, observations: &mut Vec<Observation>) {
    match policy {
        EnvPolicy::Clear => {
            cmd.env_clear();
        }
        EnvPolicy::AllowList(pairs) => {
            cmd.env_clear();
            for (key, value) in pairs {
                cmd.env(key, value);
                observations.push(Observation::EnvRecorded {
                    key: key.clone(),
                    value: value.clone(),
                });
            }
        }
        EnvPolicy::InheritWith(pairs) => {
            for (key, value) in pairs {
                cmd.env(key, value);
                observations.push(Observation::EnvRecorded {
                    key: key.clone(),
                    value: value.clone(),
                });
            }
        }
    }
}

fn drain_reader<R: Read + Send + 'static>(
    mut reader: R,
    kind: StreamKind,
    limit: usize,
    tx: mpsc::Sender<(StreamKind, CapturedStream)>,
) {
    let mut kept: Vec<u8> = Vec::with_capacity(limit.min(64 * 1024));
    let mut total: u64 = 0;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if kept.len() < limit {
                    let take = (limit - kept.len()).min(n);
                    kept.extend_from_slice(&chunk[..take]);
                }
            }
            Err(_) => break,
        }
    }
    let _ = tx.send((
        kind,
        CapturedStream {
            bytes: kept,
            total_bytes: total,
            truncated: total > limit as u64,
        },
    ));
}

fn apply_captured(
    kind: StreamKind,
    captured: CapturedStream,
    observations: &mut Vec<Observation>,
    outcome: &mut RunOutcome,
) {
    match kind {
        StreamKind::Stdout => {
            observations.push(Observation::StdoutChunk {
                bytes: captured.total_bytes,
            });
            if captured.truncated {
                observations.push(Observation::OutputTruncated {
                    stream: StreamKind::Stdout,
                    kept_bytes: captured.bytes.len() as u64,
                    total_bytes: captured.total_bytes,
                });
            } else {
                observations.push(Observation::OutputComplete {
                    stream: StreamKind::Stdout,
                    total_bytes: captured.total_bytes,
                });
            }
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stdout,
            });
            outcome.stdout = captured;
        }
        StreamKind::Stderr => {
            observations.push(Observation::StderrChunk {
                bytes: captured.total_bytes,
            });
            if captured.truncated {
                observations.push(Observation::OutputTruncated {
                    stream: StreamKind::Stderr,
                    kept_bytes: captured.bytes.len() as u64,
                    total_bytes: captured.total_bytes,
                });
            } else {
                observations.push(Observation::OutputComplete {
                    stream: StreamKind::Stderr,
                    total_bytes: captured.total_bytes,
                });
            }
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stderr,
            });
            outcome.stderr = captured;
        }
        StreamKind::Stdin => {}
    }
}

fn collect_streams(
    stream_rx: &mpsc::Receiver<(StreamKind, CapturedStream)>,
    reader_handles: Vec<std::thread::JoinHandle<()>>,
    observations: &mut Vec<Observation>,
    outcome: &mut RunOutcome,
) {
    let expected = reader_handles.len();
    let mut received = 0usize;
    while received < expected {
        match stream_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((kind, captured)) => {
                received += 1;
                apply_captured(kind, captured, observations, outcome);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if received >= expected {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    while let Ok((kind, captured)) = stream_rx.try_recv() {
        apply_captured(kind, captured, observations, outcome);
    }
    for handle in reader_handles {
        let _ = handle.join();
    }
}

fn try_wait_once(
    child: &mut std::process::Child,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    child.try_wait()
}

/// Spawn `spec` with independent stdout/stderr draining, explicit stdin
/// policy, and a hard deadline. Never uses unbounded waits without a bound.
pub fn run(spec: &CommandSpec, bounds: &OutputBounds) -> RunOutcome {
    let started = Instant::now();
    let deadline_ms = spec.deadline.timeout_ms;
    let mut observations: Vec<Observation> = Vec::new();
    let mut outcome = RunOutcome {
        observations: Vec::new(),
        exit_cause: None,
        stdout: CapturedStream::default(),
        stderr: CapturedStream::default(),
        timed_out: false,
        cleanup: CleanupOutcome::default(),
        elapsed_ms: 0,
        deadline_ms,
        bounds: *bounds,
        harness_failed: false,
        pid: None,
    };

    if spec.deadline.is_zero() {
        observations.push(Observation::HarnessFailed {
            phase: "spawn".into(),
            detail: "deadline must be > 0".into(),
        });
        outcome.observations = observations;
        outcome.harness_failed = true;
        outcome.elapsed_ms = started.elapsed().as_millis() as u64;
        return outcome;
    }

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.argv);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    apply_env(&mut cmd, &spec.env, &mut observations);

    let use_pipe_stdin = !matches!(spec.stdin, StdinSpec::Inherited);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if use_pipe_stdin {
        cmd.stdin(Stdio::piped());
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            observations.push(Observation::HarnessFailed {
                phase: "spawn".into(),
                detail: err.to_string(),
            });
            outcome.observations = observations;
            outcome.harness_failed = true;
            outcome.elapsed_ms = started.elapsed().as_millis() as u64;
            return outcome;
        }
    };

    let pid = child.id();
    outcome.pid = Some(pid);
    observations.push(Observation::ProcessSpawned { pid });

    // Stdin policy first: Closed drops the writer (EOF); Bytes writes then drops.
    match &spec.stdin {
        StdinSpec::Inherited => {}
        StdinSpec::Closed => {
            drop(child.stdin.take());
            observations.push(Observation::StdinClosed);
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stdin,
            });
        }
        StdinSpec::Bytes(bytes) => {
            if let Some(mut stdin) = child.stdin.take() {
                let write_result = stdin.write_all(bytes).and_then(|_| stdin.flush());
                let wrote = match write_result {
                    Ok(()) => bytes.len() as u64,
                    Err(err) => {
                        observations.push(Observation::HarnessFailed {
                            phase: "stdin".into(),
                            detail: err.to_string(),
                        });
                        0
                    }
                };
                drop(stdin);
                observations.push(Observation::StdinWrote { bytes: wrote });
                observations.push(Observation::DescriptorClosed {
                    stream: StreamKind::Stdin,
                });
            }
        }
    }

    let (stream_tx, stream_rx) = mpsc::channel::<(StreamKind, CapturedStream)>();
    let stdout_limit = bounds.max_stdout_bytes;
    let stderr_limit = bounds.max_stderr_bytes;

    let mut reader_handles = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let tx = stream_tx.clone();
        reader_handles.push(std::thread::spawn(move || {
            drain_reader(stdout, StreamKind::Stdout, stdout_limit, tx)
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = stream_tx.clone();
        reader_handles.push(std::thread::spawn(move || {
            drain_reader(stderr, StreamKind::Stderr, stderr_limit, tx)
        }));
    }
    drop(stream_tx);

    let wait_deadline = started + spec.deadline.duration();
    let mut exit_cause: Option<ExitCause> = None;
    let mut timed_out = false;
    let mut cleanup = CleanupOutcome::default();

    loop {
        match try_wait_once(&mut child) {
            Ok(Some(status)) => {
                exit_cause = Some(ExitCause::from_exit_status(status));
                break;
            }
            Ok(None) => {
                if Instant::now() >= wait_deadline {
                    timed_out = true;
                    break;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(err) => {
                observations.push(Observation::HarnessFailed {
                    phase: "wait".into(),
                    detail: err.to_string(),
                });
                outcome.harness_failed = true;
                break;
            }
        }
    }

    if timed_out {
        observations.push(Observation::TimeoutObserved {
            elapsed_ms: started.elapsed().as_millis() as u64,
            deadline_ms,
        });
        observations.push(Observation::ProcessStillAlive { pid });

        cleanup.attempted = true;
        let kill_succeeded = child.kill().is_ok();
        cleanup.kill_succeeded = kill_succeeded;
        let mut reaped = false;
        let reap_deadline = Instant::now() + DEFAULT_REAP_TIMEOUT;
        loop {
            match try_wait_once(&mut child) {
                Ok(Some(status)) => {
                    reaped = true;
                    // Keep TimeoutKill as the scenario cause; the kill exit
                    // status is cleanup evidence, not a portable fixture code.
                    let _ = status;
                    break;
                }
                Ok(None) => {
                    if Instant::now() >= reap_deadline {
                        break;
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(_) => break,
            }
        }
        cleanup.reaped = reaped;
        cleanup.detail = format!(
            "kill_succeeded={kill_succeeded} reaped={reaped} reap_timeout_ms={}",
            DEFAULT_REAP_TIMEOUT.as_millis()
        );
        exit_cause = Some(ExitCause::TimeoutKill);
        observations.push(Observation::CleanupAttempted {
            method: "kill_then_bounded_reap".into(),
            kill_succeeded,
            reaped,
        });
    }

    collect_streams(&stream_rx, reader_handles, &mut observations, &mut outcome);

    outcome.exit_cause = if exit_cause.is_some() {
        exit_cause
    } else if timed_out {
        Some(ExitCause::TimeoutKill)
    } else {
        None
    };
    if let Some(cause) = outcome.exit_cause.clone() {
        observations.push(Observation::ExitObserved { cause });
    }
    outcome.timed_out = timed_out;
    outcome.cleanup = cleanup;
    outcome.observations = observations;
    outcome.elapsed_ms = started.elapsed().as_millis() as u64;
    outcome
}
