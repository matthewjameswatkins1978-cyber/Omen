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
