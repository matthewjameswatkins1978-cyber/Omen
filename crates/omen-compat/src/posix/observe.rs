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
