//! Typed POSIX observations (facts only). Judgment lives in `judge`.

use crate::observation::Observation;
use crate::posix::harness::TermiosSnapshot;

/// Process identity facts from OS observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosixProcessIdentity {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub pgrp: Option<u32>,
    pub session_id: Option<u32>,
    pub source: String,
}

impl PosixProcessIdentity {
    pub fn observation(&self) -> Observation {
        Observation::PosixProcessIdentity {
            pid: self.pid,
            ppid: self.ppid,
            pgrp: self.pgrp,
            session_id: self.session_id,
            source: self.source.clone(),
        }
    }
}

/// Observe process identity for `pid` without synthesizing topology.
///
/// Consequential topology (`pgrp`, `session_id`) comes from direct POSIX
/// process APIs (`getpgid` / `getsid`). Linux `/proc/<pid>/stat` may
/// corroborate when present. Failed observation leaves `None`; callers must
/// never derive `pgrp`/`session_id` from `pid`. Establishing a topology
/// (setsid / TIOCSCTTY in the harness) is not observation.
pub fn observe_shell_identity(pid: u32) -> PosixProcessIdentity {
    let mut sources: Vec<&str> = Vec::new();
    // rustix::Pid::from_raw asserts non-negative; only convert in-range pids.
    let raw = i32::try_from(pid)
        .ok()
        .filter(|&p| p > 0)
        .and_then(rustix::process::Pid::from_raw);

    // Reassigned only by Linux /proc corroboration; never on other Unixes.
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut pgrp = raw
        .and_then(|p| rustix::process::getpgid(Some(p)).ok())
        .map(|p| p.as_raw_nonzero().get() as u32);
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut session_id = raw
        .and_then(|p| rustix::process::getsid(Some(p)).ok())
        .map(|p| p.as_raw_nonzero().get() as u32);
    if pgrp.is_some() && session_id.is_some() {
        sources.push("rustix_getpgid_getsid");
    } else if pgrp.is_some() {
        sources.push("rustix_getpgid");
    } else if session_id.is_some() {
        sources.push("rustix_getsid");
    }

    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut ppid = None;
    #[cfg(target_os = "linux")]
    {
        if let Some(proc_id) = crate::posix::linux_proc::read_process_identity(pid) {
            if pgrp.is_none() {
                pgrp = proc_id.pgrp;
            }
            if session_id.is_none() {
                session_id = proc_id.session_id;
            }
            ppid = proc_id.ppid;
            sources.push("linux_proc_stat");
        }
    }

    let source = if sources.is_empty() {
        "identity_observation_failed".into()
    } else {
        sources.join("+")
    };
    PosixProcessIdentity {
        pid,
        ppid,
        pgrp,
        session_id,
        source,
    }
}

/// Derive controlling-terminal status from observed terminal session vs
/// observed process session.
///
/// `Some(true)` only when both observations succeeded and match. Failed or
/// non-distinguishing observations yield `None` — never a hard-coded true.
/// Unequal sessions are not over-interpreted as proven non-controlling
/// without a dedicated negative observation.
pub fn derive_controlling_terminal(
    terminal_session: Option<u32>,
    process_session: Option<u32>,
) -> Option<bool> {
    match (terminal_session, process_session) {
        (Some(t), Some(p)) if t == p => Some(true),
        _ => None,
    }
}

/// Signal-mask evidence from a controlled fixture.
///
/// `available == false` means the platform/source could not measure masks.
/// That is never encoded as empty measured sets (D2-018).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalMaskEvidence {
    pub available: bool,
    pub source: String,
    pub blocked: Option<Vec<String>>,
    pub ignored: Option<Vec<String>>,
    pub caught: Option<Vec<String>>,
}

impl SignalMaskEvidence {
    /// Explicitly unavailable — not an empty measurement.
    pub fn unavailable(source: impl Into<String>) -> Self {
        Self {
            available: false,
            source: source.into(),
            blocked: None,
            ignored: None,
            caught: None,
        }
    }

    /// Measured sets from a named source (empty vectors are semantic values).
    pub fn measured(
        source: impl Into<String>,
        blocked: Vec<String>,
        ignored: Vec<String>,
        caught: Vec<String>,
    ) -> Self {
        Self {
            available: true,
            source: source.into(),
            blocked: Some(blocked),
            ignored: Some(ignored),
            caught: Some(caught),
        }
    }

    pub fn observation(&self) -> Observation {
        Observation::PosixSignalMaskReport {
            fixture: "omen-gremlin".into(),
            available: self.available,
            blocked: self.blocked.clone(),
            ignored: self.ignored.clone(),
            source: self.source.clone(),
        }
    }
}

/// Parse fixture signal-mask availability from a transcript.
///
/// Requires an explicit `signal_masks_available` boolean. Missing field or
/// unavailable payload yields unavailable evidence — never empty measured
/// sets.
pub fn parse_signal_mask_evidence(text: &str) -> SignalMaskEvidence {
    for line in text.lines().rev() {
        let line = line.trim();
        let json_start = match line.find('{') {
            Some(i) => i,
            None => continue,
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line[json_start..]) else {
            continue;
        };
        let Some(available) = v.get("signal_masks_available").and_then(|b| b.as_bool()) else {
            continue;
        };
        if !available {
            let source = v
                .get("signal_masks_source")
                .and_then(|s| s.as_str())
                .unwrap_or("unavailable");
            return SignalMaskEvidence::unavailable(source);
        }
        // Available but blocked is not an array (null/missing): not a
        // measurement — refuse to invent empty measured sets (D2-018).
        if !v.get("blocked").is_some_and(|b| b.is_array()) {
            return SignalMaskEvidence::unavailable(
                "signal_masks_available true but blocked is not an array",
            );
        }
        let source = v
            .get("signal_masks_source")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        let arr = |key: &str| -> Vec<String> {
            v.get(key)
                .and_then(|a| a.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        return SignalMaskEvidence::measured(source, arr("blocked"), arr("ignored"), arr("caught"));
    }
    SignalMaskEvidence::unavailable("signal_masks_available missing from fixture report")
}

/// Terminal ownership facts (fg pgrp, session, winsize, selected termios).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosixTerminalState {
    pub foreground_pgrp: Option<u32>,
    pub session_id: Option<u32>,
    pub is_controlling_terminal: Option<bool>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub termios: Option<TermiosSnapshot>,
    pub source: String,
}

impl PosixTerminalState {
    pub fn observation(&self) -> Observation {
        Observation::PosixTerminalState {
            foreground_pgrp: self.foreground_pgrp,
            session_id: self.session_id,
            is_controlling_terminal: self.is_controlling_terminal,
            rows: self.rows,
            cols: self.cols,
            icanon: self.termios.map(|t| t.icanon),
            echo: self.termios.map(|t| t.echo),
            isig: self.termios.map(|t| t.isig),
            source: self.source.clone(),
        }
    }
}

/// Wait-state facts for a direct child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PosixWaitState {
    pub pid: u32,
    pub exited: bool,
    pub signaled: bool,
    pub stopped: bool,
    pub continued: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub source: String,
}

impl PosixWaitState {
    pub fn observation(&self) -> Observation {
        Observation::PosixWaitState {
            pid: self.pid,
            exited: self.exited,
            signaled: self.signaled,
            stopped: self.stopped,
            continued: self.continued,
            exit_code: self.exit_code,
            signal: self.signal,
            source: self.source.clone(),
        }
    }
}

/// Failure parsing a fixture identity report. Never converted into a
/// synthetic [`PosixProcessIdentity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseEvidenceError {
    /// No fixture report line in the transcript.
    Missing,
    /// Report line present but unusable (bad JSON or missing pid/pgrp/sid).
    Malformed { detail: String },
}

impl std::fmt::Display for ParseEvidenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseEvidenceError::Missing => write!(f, "fixture posix-report missing"),
            ParseEvidenceError::Malformed { detail } => {
                write!(f, "fixture posix-report malformed: {detail}")
            }
        }
    }
}

/// Parse a fixture `posix-report` line into identity facts.
///
/// Requires a parseable JSON line with positive `pid`, `pgrp`, and `sid`.
/// Missing or malformed input returns [`ParseEvidenceError`]; callers must
/// never invent pid/pgrp/sid from shell values.
pub fn parse_posix_report(text: &str) -> Result<PosixProcessIdentity, ParseEvidenceError> {
    for line in text.lines().rev() {
        let line = line.trim();
        if !line.contains("\"fixture\":\"posix-report\"") {
            continue;
        }
        // Terminal prompts/ANSI may prefix the JSON object; parse from first '{'.
        let json_start = line
            .find('{')
            .ok_or_else(|| ParseEvidenceError::Malformed {
                detail: "no JSON object start".into(),
            })?;
        let json_slice = &line[json_start..];
        let v: serde_json::Value =
            serde_json::from_str(json_slice).map_err(|e| ParseEvidenceError::Malformed {
                detail: format!("json: {e}"),
            })?;
        let pid =
            v["pid"]
                .as_u64()
                .filter(|&p| p > 0)
                .ok_or_else(|| ParseEvidenceError::Malformed {
                    detail: "pid missing or non-positive".into(),
                })? as u32;
        let pgrp =
            v["pgrp"]
                .as_i64()
                .filter(|&p| p > 0)
                .ok_or_else(|| ParseEvidenceError::Malformed {
                    detail: "pgrp missing or non-positive".into(),
                })? as u32;
        let sid =
            v["sid"]
                .as_i64()
                .filter(|&p| p > 0)
                .ok_or_else(|| ParseEvidenceError::Malformed {
                    detail: "sid missing or non-positive".into(),
                })? as u32;
        let ppid = v["ppid"].as_i64().map(|x| x as u32).filter(|&x| x > 0);
        return Ok(PosixProcessIdentity {
            pid,
            ppid,
            pgrp: Some(pgrp),
            session_id: Some(sid),
            source: "fixture_posix_report".into(),
        });
    }
    Err(ParseEvidenceError::Missing)
}

/// Explicit prior distinct foreground-handoff evidence.
///
/// Reacquisition and distinct-job-ownership claims require this chain:
/// shell pgrp known, job pgrp known, `job_pgrp != shell_pgrp`, and terminal
/// foreground while the job ran equaled the job pgrp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffEvidence {
    pub shell_pgrp: u32,
    pub job_pgrp: u32,
    pub terminal_fg_during_job: u32,
}

impl HandoffEvidence {
    /// True only when the full distinct-handoff chain is present.
    pub fn is_proven(&self) -> bool {
        self.shell_pgrp != self.job_pgrp && self.terminal_fg_during_job == self.job_pgrp
    }

    /// Build from optional observations; `None` if any link is missing.
    pub fn try_from_observations(
        shell_pgrp: Option<u32>,
        job_pgrp: Option<u32>,
        terminal_fg_during_job: Option<u32>,
    ) -> Option<Self> {
        Some(Self {
            shell_pgrp: shell_pgrp?,
            job_pgrp: job_pgrp?,
            terminal_fg_during_job: terminal_fg_during_job?,
        })
    }
}

/// Process-stopped fact from an independent source (e.g. Linux `/proc`).
///
/// This is **not** evidence that Omen's wait/job-control path observed
/// STOPPED. It never feeds [`crate::judge_wait_observes_stopped`] PASS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStoppedObserved {
    pub pid: u32,
    pub source: String,
}

impl JobStoppedObserved {
    pub fn observation(&self) -> Observation {
        Observation::JobStoppedObserved {
            pid: self.pid,
            source: self.source.clone(),
        }
    }
}

/// Whether terminal-generated SIGINT receipt was independently observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigintReceiptObservation {
    /// Fixture (or other independent source) reported actual SIGINT receipt.
    Observed { source: String },
    /// VINTR was injected but no receipt evidence appeared within the bound.
    InjectedNotObserved,
    /// Receipt cannot be observed on this path/platform.
    Unavailable { reason: String },
}
