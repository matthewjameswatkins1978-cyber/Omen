use crate::core::{CommandSpec, EnvPolicy, EvidenceGrade, ExitCause, OutputBounds, StdinSpec};
use crate::failure::StreamSummary;
use crate::invariant::{InvariantId, InvariantOutcome, InvariantResult};
use crate::observation::{Observation, StreamKind};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// Bounded cleanup after scenario deadline (non-waiting kill init + bounded reap).
pub const CLEANUP_BOUND: Duration = Duration::from_millis(500);
/// One shared post-root window covering stdin/stdout/stderr completion and
/// task cancellation/abort acknowledgement. Never reset per task.
pub const IO_COMPLETION_GRACE: Duration = Duration::from_millis(300);
/// Small scheduling tolerance added to the declared wall-clock promise.
pub const SCHEDULING_TOLERANCE_MS: u64 = 75;

/// Canonical declared maximum wall-clock formula.
///
/// `deadline_ms + CLEANUP_BOUND + IO_COMPLETION_GRACE + SCHEDULING_TOLERANCE_MS`
///
/// `IO_COMPLETION_GRACE` is one shared budget for ALL post-root stdin,
/// stdout, and stderr completion bookkeeping (including abort acknowledgement).
/// No undocumented additive per-task waits.
pub fn declared_max_wall_ms(deadline_ms: u64) -> u64 {
    deadline_ms
        + CLEANUP_BOUND.as_millis() as u64
        + IO_COMPLETION_GRACE.as_millis() as u64
        + SCHEDULING_TOLERANCE_MS
}

/// Bounded capture for one stream.
///
/// Truncation, EOF, and drain cancellation are distinct facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CapturedStream {
    pub bytes: Vec<u8>,
    /// Bytes observed on the wire before capture stopped.
    pub total_bytes: u64,
    /// More bytes arrived than retained.
    pub truncated: bool,
    /// Pipe EOF was observed on this stream.
    pub eof_observed: bool,
    /// Drain finished without being cancelled at the grace bound.
    pub drain_complete: bool,
    /// Drain was stopped at the declared grace bound (EOF not proven).
    pub drain_timed_out: bool,
}

impl CapturedStream {
    pub fn as_lossy_string(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    pub fn summary(&self) -> StreamSummary {
        StreamSummary::from_captured(
            self.bytes.len() as u64,
            self.total_bytes,
            self.truncated,
            self.eof_observed,
            self.drain_timed_out,
        )
    }
}

/// Truthful record of post-deadline cleanup of the **root** process only.
///
/// Distinguishes kill *initiation* from process termination. Does not claim
/// descendant-tree termination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CleanupOutcome {
    /// Kill initiation was attempted after the scenario deadline.
    pub kill_attempted: bool,
    /// Non-waiting kill request was accepted (`start_kill` Ok). Not proof
    /// that the process has already terminated.
    pub kill_initiated: bool,
    /// Root process was reaped within `CLEANUP_BOUND`.
    pub root_reaped: bool,
    /// Root-process cleanup only; descendants are not proven gone.
    pub root_only: bool,
    pub detail: String,
}

/// Structured result of one bounded run.
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
    pub declared_max_wall_ms: u64,
    pub bounds: OutputBounds,
    pub harness_failed: bool,
    pub pid: Option<u32>,
}

enum StdinDelivery {
    Closed,
    Wrote(u64),
    Failed { bytes: u64, detail: String },
    TimedOut { bytes: u64 },
}

async fn deliver_stdin(
    mut stdin: Option<tokio::process::ChildStdin>,
    spec: &StdinSpec,
    budget: Duration,
) -> StdinDelivery {
    match spec {
        StdinSpec::Inherited => StdinSpec::inherited_delivery(),
        StdinSpec::Closed => {
            drop(stdin.take());
            StdinDelivery::Closed
        }
        StdinSpec::Bytes(bytes) => {
            let Some(mut sink) = stdin.take() else {
                return StdinDelivery::Closed;
            };
            let attempted = bytes.len() as u64;
            match tokio::time::timeout(budget, async {
                sink.write_all(bytes).await?;
                sink.flush().await?;
                sink.shutdown().await?;
                Ok::<(), std::io::Error>(())
            })
            .await
            {
                Ok(Ok(())) => StdinDelivery::Wrote(attempted),
                Ok(Err(err)) => StdinDelivery::Failed {
                    bytes: attempted,
                    detail: err.to_string(),
                },
                Err(_) => {
                    drop(sink);
                    StdinDelivery::TimedOut { bytes: attempted }
                }
            }
        }
    }
}

impl StdinSpec {
    fn inherited_delivery() -> StdinDelivery {
        StdinDelivery::Closed
    }
}

async fn drain_stream<R>(mut reader: R, limit: usize) -> CapturedStream
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut kept: Vec<u8> = Vec::with_capacity(limit.min(64 * 1024));
    let mut total: u64 = 0;
    let mut buf = [0u8; 16 * 1024];
    let mut eof = false;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                eof = true;
                break;
            }
            Ok(n) => {
                total += n as u64;
                if kept.len() < limit {
                    let take = (limit - kept.len()).min(n);
                    kept.extend_from_slice(&buf[..take]);
                }
            }
            Err(_) => break,
        }
    }
    CapturedStream {
        bytes: kept,
        total_bytes: total,
        truncated: total > limit as u64,
        eof_observed: eof,
        drain_complete: eof,
        drain_timed_out: false,
    }
}

/// Wait for a task until a **shared absolute deadline**, then abort.
///
/// Abort acknowledgement uses only the remaining time on the same deadline —
/// never a fresh per-task allowance outside the declared budget.
async fn finish_task_until<T>(handle: &mut JoinHandle<T>, deadline: Instant) -> Option<T> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        handle.abort();
        let ack = deadline.saturating_duration_since(Instant::now());
        if !ack.is_zero() {
            let _ = tokio::time::timeout(ack, &mut *handle).await;
        }
        return None;
    }
    match tokio::time::timeout(remaining, &mut *handle).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) => None,
        Err(_) => {
            handle.abort();
            let ack = deadline.saturating_duration_since(Instant::now());
            if !ack.is_zero() {
                let _ = tokio::time::timeout(ack, &mut *handle).await;
            }
            None
        }
    }
}

async fn finish_optional_until<T>(
    handle: &mut Option<JoinHandle<T>>,
    deadline: Instant,
) -> Option<T> {
    match handle.as_mut() {
        Some(task) => finish_task_until(task, deadline).await,
        None => None,
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
                observations.push(Observation::EnvApplied { key: key.clone() });
            }
        }
        EnvPolicy::InheritWith(pairs) => {
            for (key, value) in pairs {
                cmd.env(key, value);
                observations.push(Observation::EnvApplied { key: key.clone() });
            }
        }
    }
}

fn record_stream_observations(
    kind: StreamKind,
    captured: &CapturedStream,
    observations: &mut Vec<Observation>,
) {
    match kind {
        StreamKind::Stdout => observations.push(Observation::StdoutChunk {
            bytes: captured.total_bytes,
        }),
        StreamKind::Stderr => observations.push(Observation::StderrChunk {
            bytes: captured.total_bytes,
        }),
        StreamKind::Stdin => return,
    }
    if captured.truncated {
        observations.push(Observation::OutputTruncated {
            stream: kind,
            kept_bytes: captured.bytes.len() as u64,
            total_bytes: captured.total_bytes,
        });
    } else if captured.eof_observed {
        observations.push(Observation::OutputComplete {
            stream: kind,
            total_bytes: captured.total_bytes,
        });
    }
    if captured.drain_timed_out {
        observations.push(Observation::StreamDrainBoundedOut {
            stream: kind,
            total_bytes_observed: captured.total_bytes,
        });
    }
    observations.push(Observation::DescriptorClosed { stream: kind });
}

impl RunOutcome {
    pub fn stdout_lossy(&self) -> String {
        self.stdout.as_lossy_string()
    }

    pub fn stderr_lossy(&self) -> String {
        self.stderr.as_lossy_string()
    }

    /// Locate the last single-line JSON object on stdout (fixture reports).
    /// FixtureReport payloads remain limited to controlled Compat fixtures.
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

    pub fn capture_summary(stream: &CapturedStream) -> StreamSummary {
        stream.summary()
    }

    /// BOUNDED_WAIT_NO_HANG: verifies **actual elapsed wall-clock time**
    /// against the declared maximum, not merely that a status flag exists.
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

        let max = self.declared_max_wall_ms;
        if self.elapsed_ms > max {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "elapsed {}ms exceeded declared maximum {}ms (deadline {}ms)",
                    self.elapsed_ms, max, self.deadline_ms
                ),
            )
            .with_observations(self.observations.clone());
        }

        let reached_terminal = self.exit_cause.is_some() || self.timed_out;
        if reached_terminal {
            InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "elapsed {}ms within declared maximum {}ms (deadline {}ms)",
                    self.elapsed_ms, max, self.deadline_ms
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

    /// DRAIN_BOTH_STREAMS_NO_DEADLOCK: both streams captured without hanging.
    /// Does not claim EOF when drain was bounded out.
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
        let eof_note = format!(
            "stdout_eof={} stderr_eof={} stdout_drain_timed_out={} stderr_drain_timed_out={}",
            self.stdout.eof_observed,
            self.stderr.eof_observed,
            self.stdout.drain_timed_out,
            self.stderr.drain_timed_out
        );
        InvariantResult::pass(
            invariant,
            EvidenceGrade::Strong,
            format!(
                "both streams drained without hang; stdout_total={} stderr_total={} truncated_out={} truncated_err={} {eof_note}",
                self.stdout.total_bytes,
                self.stderr.total_bytes,
                self.stdout.truncated,
                self.stderr.truncated
            ),
        )
        .with_observations(self.observations.clone())
    }

    /// DESCRIPTOR_CLOSURE_EARLY_EXIT: child may exit while parent still holds
    /// write handles; the runner must still return within the declared bound.
    pub fn check_descriptor_closure_early_exit(&self) -> InvariantResult {
        let invariant = InvariantId::DescriptorClosureEarlyExit;
        if self.elapsed_ms > self.declared_max_wall_ms {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!(
                    "elapsed {}ms exceeded declared maximum {}ms after early child exit",
                    self.elapsed_ms, self.declared_max_wall_ms
                ),
            )
            .with_observations(self.observations.clone());
        }
        if self.timed_out {
            return InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                "runner hit deadline after early child exit (descriptor lifecycle)",
            )
            .with_observations(self.observations.clone());
        }
        match &self.exit_cause {
            Some(ExitCause::Code { .. }) => InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                "child exited while harness retained stream ownership; runner returned in bound",
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

    /// ENV_RECORDED: dedicated probe matched in memory.
    /// Durable reason strings never interpolate environment values.
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
        let applied = self
            .observations
            .iter()
            .any(|obs| matches!(obs, Observation::EnvApplied { key } if key == expected_key));

        match (applied, fixture_value) {
            (true, Some(fixture_val)) if fixture_val == expected_value => InvariantResult::pass(
                invariant,
                EvidenceGrade::Strong,
                "dedicated environment probe value matched",
            )
            .with_observations(self.observations.clone()),
            (true, Some(_)) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                "dedicated environment probe value mismatched",
            )
            .with_observations(self.observations.clone()),
            (true, None) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Partial,
                "harness recorded env application but fixture did not report the probe",
            ),
            (false, _) => InvariantResult::fail(
                invariant,
                EvidenceGrade::Strong,
                format!("harness never recorded EnvApplied for key {expected_key}"),
            ),
        }
    }
}

/// Spawn `spec` with concurrent bounded stdin/stdout/stderr I/O.
///
/// All external waits are bounded by the scenario deadline, cleanup bound,
/// and stream-drain grace. No unbounded join/read/write remains.
pub async fn run(spec: &CommandSpec, bounds: &OutputBounds) -> RunOutcome {
    let started = Instant::now();
    let deadline_ms = spec.deadline.timeout_ms;
    let declared_max = declared_max_wall_ms(deadline_ms);
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
        declared_max_wall_ms: declared_max,
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

    let scenario_deadline = spec.deadline.duration();
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.argv);
    cmd.kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    apply_env(&mut cmd, &spec.env, &mut observations);

    let use_pipe_stdin = !matches!(spec.stdin, StdinSpec::Inherited);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if use_pipe_stdin {
        cmd.stdin(std::process::Stdio::piped());
    }

    let mut child: Child = match cmd.spawn() {
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

    let pid = child.id().unwrap_or(0);
    outcome.pid = Some(pid);
    observations.push(Observation::ProcessSpawned { pid });

    let stdin_handle_raw = if matches!(spec.stdin, StdinSpec::Closed) {
        drop(child.stdin.take());
        observations.push(Observation::StdinClosed);
        observations.push(Observation::DescriptorClosed {
            stream: StreamKind::Stdin,
        });
        None
    } else {
        child.stdin.take()
    };
    let stdout_raw = child.stdout.take();
    let stderr_raw = child.stderr.take();

    let stdin_spec = spec.stdin.clone();
    let stdin_budget = scenario_deadline.min(Duration::from_secs(30));
    let mut stdin_task = tokio::spawn(async move {
        if matches!(stdin_spec, StdinSpec::Closed) {
            return StdinDelivery::Closed;
        }
        deliver_stdin(stdin_handle_raw, &stdin_spec, stdin_budget).await
    });

    let mut stdout_task =
        stdout_raw.map(|stdout| tokio::spawn(drain_stream(stdout, bounds.max_stdout_bytes)));
    let mut stderr_task =
        stderr_raw.map(|stderr| tokio::spawn(drain_stream(stderr, bounds.max_stderr_bytes)));

    let mut exit_cause: Option<ExitCause> = None;
    let mut timed_out = false;
    let mut cleanup = CleanupOutcome::default();

    match tokio::time::timeout(scenario_deadline, child.wait()).await {
        Ok(Ok(status)) => {
            exit_cause = Some(ExitCause::from_exit_status(status));
        }
        Ok(Err(err)) => {
            observations.push(Observation::HarnessFailed {
                phase: "wait".into(),
                detail: err.to_string(),
            });
            outcome.harness_failed = true;
        }
        Err(_) => {
            timed_out = true;
            observations.push(Observation::TimeoutObserved {
                elapsed_ms: started.elapsed().as_millis() as u64,
                deadline_ms,
            });
            observations.push(Observation::ProcessStillAlive { pid });

            cleanup.kill_attempted = true;
            cleanup.root_only = true;
            // Non-waiting kill initiation: Child::kill().await may itself wait
            // for termination and would break CLEANUP_BOUND before reap starts.
            let kill_initiated = child.start_kill().is_ok();
            cleanup.kill_initiated = kill_initiated;
            let root_reaped = matches!(
                tokio::time::timeout(CLEANUP_BOUND, child.wait()).await,
                Ok(Ok(_status))
            );
            cleanup.root_reaped = root_reaped;
            cleanup.detail = format!(
                "root_only=true kill_initiated={kill_initiated} root_reaped={root_reaped} cleanup_bound_ms={}",
                CLEANUP_BOUND.as_millis()
            );
            exit_cause = Some(ExitCause::TimeoutKill);
            observations.push(Observation::CleanupAttempted {
                method: "start_kill_then_bounded_reap_root_only".into(),
                kill_initiated,
                root_reaped,
            });
        }
    }

    // One shared absolute post-root I/O completion window for stdin + stdout +
    // stderr (including abort acknowledgement). Never reset the grace clock
    // per task; total bookkeeping fits IO_COMPLETION_GRACE exactly.
    let completion_deadline = Instant::now() + IO_COMPLETION_GRACE;
    let (stdin_done, stdout_done, stderr_done) = tokio::join!(
        finish_task_until(&mut stdin_task, completion_deadline),
        finish_optional_until(&mut stdout_task, completion_deadline),
        finish_optional_until(&mut stderr_task, completion_deadline),
    );

    match stdin_done {
        Some(StdinDelivery::Closed) => {}
        Some(StdinDelivery::Wrote(bytes)) => {
            observations.push(Observation::StdinWrote { bytes });
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stdin,
            });
        }
        Some(StdinDelivery::Failed { bytes, detail }) => {
            observations.push(Observation::StdinWrote { bytes });
            observations.push(Observation::HarnessFailed {
                phase: "stdin".into(),
                detail,
            });
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stdin,
            });
        }
        Some(StdinDelivery::TimedOut { bytes }) => {
            observations.push(Observation::StdinDeliveryBounded {
                bytes_attempted: bytes,
                completed: false,
            });
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stdin,
            });
        }
        None => {
            observations.push(Observation::StdinDeliveryBounded {
                bytes_attempted: spec.stdin.byte_length().unwrap_or(0) as u64,
                completed: false,
            });
            observations.push(Observation::DescriptorClosed {
                stream: StreamKind::Stdin,
            });
        }
    }

    if let Some(captured) = stdout_done {
        record_stream_observations(StreamKind::Stdout, &captured, &mut observations);
        outcome.stdout = captured;
    } else if stdout_task.is_some() {
        let partial = CapturedStream {
            bytes: Vec::new(),
            total_bytes: 0,
            truncated: false,
            eof_observed: false,
            drain_complete: false,
            drain_timed_out: true,
        };
        record_stream_observations(StreamKind::Stdout, &partial, &mut observations);
        outcome.stdout = partial;
    }

    if let Some(captured) = stderr_done {
        record_stream_observations(StreamKind::Stderr, &captured, &mut observations);
        outcome.stderr = captured;
    } else if stderr_task.is_some() {
        let partial = CapturedStream {
            bytes: Vec::new(),
            total_bytes: 0,
            truncated: false,
            eof_observed: false,
            drain_complete: false,
            drain_timed_out: true,
        };
        record_stream_observations(StreamKind::Stderr, &partial, &mut observations);
        outcome.stderr = partial;
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_max_wall_ms_formula() {
        assert_eq!(
            declared_max_wall_ms(400),
            400 + CLEANUP_BOUND.as_millis() as u64
                + IO_COMPLETION_GRACE.as_millis() as u64
                + SCHEDULING_TOLERANCE_MS
        );
        // One shared grace budget — not 3x per stream.
        assert_eq!(IO_COMPLETION_GRACE, Duration::from_millis(300));
    }

    #[test]
    fn runner_source_never_uses_unbounded_child_kill_await() {
        let src = include_str!("runner.rs");
        // Build needle so this test does not embed the forbidden form literally.
        let forbidden: String = ["child", ".kill()", ".await"].join("");
        assert!(
            !src.contains(&forbidden),
            "timeout cleanup must use start_kill() + bounded reap, not Child kill-await"
        );
        assert!(
            src.contains("start_kill()"),
            "non-waiting kill initiation must remain present"
        );
    }

    #[test]
    fn bounded_wait_checks_elapsed_not_just_flags() {
        let deadline = 400u64;
        let max = declared_max_wall_ms(deadline);
        let mut ok = RunOutcome {
            observations: vec![],
            exit_cause: Some(ExitCause::code(0)),
            stdout: CapturedStream::default(),
            stderr: CapturedStream::default(),
            timed_out: false,
            cleanup: CleanupOutcome::default(),
            elapsed_ms: max - 1,
            deadline_ms: deadline,
            declared_max_wall_ms: max,
            bounds: OutputBounds::default(),
            harness_failed: false,
            pid: Some(1),
        };
        assert_eq!(
            ok.check_bounded_wait_no_hang().outcome,
            InvariantOutcome::Pass
        );

        ok.elapsed_ms = max + 1;
        assert_eq!(
            ok.check_bounded_wait_no_hang().outcome,
            InvariantOutcome::Fail
        );
        assert!(
            ok.check_bounded_wait_no_hang()
                .reason
                .contains("exceeded declared maximum")
        );
    }
}
