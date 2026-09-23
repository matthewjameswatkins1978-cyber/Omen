//! Bounded POSIX PTY harness owned by omen-compat.
//!
//! Establishes an explicit PTY topology: master in the harness process,
//! slave session created by the child (`setsid` + controlling terminal +
//! foreground pgrp) before exec. Opening a slave alone is never treated as
//! proof of controlling-terminal ownership.

use crate::observation::Observation;
use std::io::{Read, Write};
use std::os::fd::{AsFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Poll slice for PTY reads (never an unbounded blocking read).
pub const PTY_POLL_SLICE_MS: u64 = 50;
/// Retained transcript cap (bounded capture model, mirrors M0 stream bounds).
pub const PTY_TRANSCRIPT_CAP: usize = 64 * 1024;

/// Bounded PTY transcript capture.
#[derive(Debug, Clone, Default)]
pub struct PtyTranscript {
    pub retained: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
    pub eof_or_hangup: bool,
    pub bounded_out: bool,
}

impl PtyTranscript {
    pub fn as_lossy(&self) -> String {
        String::from_utf8_lossy(&self.retained).into_owned()
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.retained
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
    }

    fn push(&mut self, chunk: &[u8]) {
        self.total_bytes += chunk.len() as u64;
        if self.retained.len() < PTY_TRANSCRIPT_CAP {
            let room = PTY_TRANSCRIPT_CAP - self.retained.len();
            let take = room.min(chunk.len());
            self.retained.extend_from_slice(&chunk[..take]);
            if take < chunk.len() {
                self.truncated = true;
            }
        } else {
            self.truncated = true;
        }
    }
}

/// Explicit winsize (test data, not semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyWinsize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtyWinsize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

/// Selected canonical termios snapshot (consequential flags only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TermiosSnapshot {
    pub icanon: bool,
    pub echo: bool,
    pub isig: bool,
}

/// Live PTY topology with one direct session child.
pub struct PtySession {
    master: OwnedFd,
    /// Kept open for tcgetpgrp / tcgetattr without re-acquiring ctty.
    observe_slave: OwnedFd,
    slave_path: PathBuf,
    child: Child,
    child_pid: u32,
    transcript: PtyTranscript,
    started: Instant,
    deadline: Duration,
}

fn io_err(phase: &str, err: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("{phase}: {err}"))
}

impl PtySession {
    /// Spawn `program` as a new session leader with this PTY as controlling
    /// terminal and itself as the foreground process group.
    pub fn spawn_session_child(
        program: impl AsRef<Path>,
        args: &[String],
        env_clear: bool,
        env: &[(String, String)],
        cwd: Option<&Path>,
        winsize: PtyWinsize,
        deadline: Duration,
    ) -> std::io::Result<Self> {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        use rustix::termios::{Winsize, tcsetwinsize};

        if deadline.is_zero() {
            return Err(std::io::Error::other("deadline must be > 0"));
        }

        let master =
            openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(|e| io_err("openpt", e))?;
        grantpt(&master).map_err(|e| io_err("grantpt", e))?;
        unlockpt(&master).map_err(|e| io_err("unlockpt", e))?;
        let name_c = ptsname(&master, Vec::new()).map_err(|e| io_err("ptsname", e))?;
        let name_bytes = name_c.to_bytes().to_vec();
        let slave_path =
            PathBuf::from(String::from_utf8(name_bytes).map_err(|e| io_err("ptsname_utf8", e))?);

        let ws = Winsize {
            ws_row: winsize.rows,
            ws_col: winsize.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        tcsetwinsize(&master, ws).map_err(|e| io_err("tcsetwinsize_master", e))?;

        // Observation fd: O_NOCTTY so the harness never acquires the slave ctty.
        let observe_slave = rustix::fs::open(
            slave_path.as_os_str().as_encoded_bytes(),
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY,
            rustix::fs::Mode::empty(),
        )
        .map_err(|e| io_err("open_observe_slave", e))?;
        tcsetwinsize(&observe_slave, ws).map_err(|e| io_err("tcsetwinsize_slave", e))?;

        let slave_io = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&slave_path)
            .map_err(|e| io_err("open_slave_io", e))?;
        let slave_in = slave_io
            .try_clone()
            .map_err(|e| io_err("clone_slave_in", e))?;
        let slave_out = slave_io
            .try_clone()
            .map_err(|e| io_err("clone_slave_out", e))?;

        let mut cmd = Command::new(program.as_ref());
        cmd.args(args);
        if env_clear {
            cmd.env_clear();
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::from(slave_in));
        cmd.stdout(Stdio::from(slave_out));
        cmd.stderr(Stdio::from(slave_io));

        // Establish session + controlling terminal + foreground pgrp in the
        // child after fork, before exec. Observed independently afterward.
        unsafe {
            cmd.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                let stdin = std::io::stdin();
                rustix::process::ioctl_tiocsctty(&stdin).map_err(std::io::Error::from)?;
                let pid = rustix::process::getpid();
                rustix::termios::tcsetpgrp(&stdin, pid).map_err(std::io::Error::from)?;
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| io_err("spawn", e))?;
        let child_pid = child.id();

        set_nonblocking(master.as_fd()).map_err(|e| io_err("nonblock_master", e))?;

        Ok(Self {
            master,
            observe_slave,
            slave_path,
            child,
            child_pid,
            transcript: PtyTranscript::default(),
            started: Instant::now(),
            deadline,
        })
    }

    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    pub fn slave_path(&self) -> &Path {
        &self.slave_path
    }

    pub fn transcript(&self) -> &PtyTranscript {
        &self.transcript
    }

    pub fn master_fd(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }

    pub fn observe_slave_fd(&self) -> BorrowedFd<'_> {
        self.observe_slave.as_fd()
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_sub(self.started.elapsed())
    }

    /// Poll-read the master until `budget` elapses, predicate matches, EOF,
    /// or the scenario deadline. Never a single unbounded read.
    pub fn pump_until(
        &mut self,
        predicate: impl Fn(&PtyTranscript) -> bool,
        budget: Duration,
    ) -> Result<PtyTranscript, std::io::Error> {
        let outer = Instant::now() + budget.min(self.remaining());
        let mut buf = [0u8; 4096];
        loop {
            if predicate(&self.transcript) {
                return Ok(self.transcript.clone());
            }
            let now = Instant::now();
            if now >= outer || self.remaining().is_zero() {
                self.transcript.bounded_out = true;
                return Ok(self.transcript.clone());
            }
            let slice = (outer - now).min(Duration::from_millis(PTY_POLL_SLICE_MS));
            match poll_readable(self.master.as_fd(), slice)? {
                PollOutcome::Timeout => continue,
                PollOutcome::Hangup => {
                    self.transcript.eof_or_hangup = true;
                    if predicate(&self.transcript) {
                        return Ok(self.transcript.clone());
                    }
                    self.transcript.bounded_out = true;
                    return Ok(self.transcript.clone());
                }
                PollOutcome::Readable => {
                    let n = rustix::io::read(self.master.as_fd(), &mut buf)
                        .map_err(|e| io_err("read_master", e))?;
                    if n == 0 {
                        self.transcript.eof_or_hangup = true;
                        if predicate(&self.transcript) {
                            return Ok(self.transcript.clone());
                        }
                        self.transcript.bounded_out = true;
                        return Ok(self.transcript.clone());
                    }
                    self.transcript.push(&buf[..n]);
                    // Answer DSR cursor-position queries so line editors
                    // (reedline) do not block forever waiting for ESC[6n.
                    if self.transcript.retained.windows(4).any(|w| w == b"\x1b[6n")
                        && !self.transcript.retained.ends_with(b"R")
                    {
                        let _ = self.write_master(b"\x1b[24;80R");
                    }
                }
            }
        }
    }

    /// Wait until the transcript contains `needle` (fixture barrier).
    pub fn wait_for_text(
        &mut self,
        needle: &str,
        budget: Duration,
    ) -> Result<PtyTranscript, std::io::Error> {
        if self.transcript.contains(needle) {
            return Ok(self.transcript.clone());
        }
        self.pump_until(|t| t.contains(needle), budget)
    }

    /// Write bytes to the master (input toward the session child).
    pub fn write_master(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::os::fd::FromRawFd;
        // Duplicate so we can use Read/Write without consuming the OwnedFd.
        let raw = rustix::io::dup(self.master.as_fd())
            .map_err(|e| io_err("dup_master_write", e))?
            .into_raw_fd();
        let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
        file.write_all(bytes)
            .map_err(|e| io_err("write_master", e))?;
        file.flush().map_err(|e| io_err("flush_master", e))
    }

    /// Inject a terminal special character (e.g. VINTR = 0x03) via the master.
    pub fn write_ctrl(&mut self, byte: u8) -> std::io::Result<()> {
        self.write_master(&[byte])
    }

    /// Observe terminal foreground pgrp via `tcgetpgrp` on the master.
    ///
    /// The harness observes from outside the child session. On Linux,
    /// `TIOCGPGRP` on an `O_NOCTTY` slave fd opened by a non-session process
    /// can yield ENOTTY; the master remains a valid observation surface.
    pub fn observe_foreground_pgrp(&self) -> std::io::Result<u32> {
        let pid =
            rustix::termios::tcgetpgrp(self.master.as_fd()).map_err(|e| io_err("tcgetpgrp", e))?;
        Ok(pid.as_raw_nonzero().get() as u32)
    }

    /// Observe the terminal's session id via `tcgetsid` (slave preferred,
    /// master fallback where the platform allows).
    pub fn observe_terminal_session(&self) -> std::io::Result<u32> {
        match rustix::termios::tcgetsid(self.observe_slave.as_fd()) {
            Ok(pid) => Ok(pid.as_raw_nonzero().get() as u32),
            Err(_) => {
                let pid = rustix::termios::tcgetsid(self.master.as_fd())
                    .map_err(|e| io_err("tcgetsid", e))?;
                Ok(pid.as_raw_nonzero().get() as u32)
            }
        }
    }

    /// Observe winsize via `tcgetwinsize`.
    pub fn observe_winsize(&self) -> std::io::Result<PtyWinsize> {
        let ws = rustix::termios::tcgetwinsize(self.observe_slave.as_fd())
            .map_err(|e| io_err("tcgetwinsize", e))?;
        Ok(PtyWinsize {
            rows: ws.ws_row,
            cols: ws.ws_col,
        })
    }

    /// Change PTY dimensions (kernel delivers SIGWINCH to the fg pgrp).
    pub fn set_winsize(&self, size: PtyWinsize) -> std::io::Result<()> {
        use rustix::termios::{Winsize, tcsetwinsize};
        let ws = Winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        tcsetwinsize(self.master.as_fd(), ws).map_err(|e| io_err("tcsetwinsize", e))
    }

    /// Selected canonical termios snapshot from the slave.
    pub fn observe_termios_snapshot(&self) -> std::io::Result<TermiosSnapshot> {
        let t = rustix::termios::tcgetattr(self.observe_slave.as_fd())
            .map_err(|e| io_err("tcgetattr", e))?;
        Ok(TermiosSnapshot {
            icanon: t.local_modes.contains(rustix::termios::LocalModes::ICANON),
            echo: t.local_modes.contains(rustix::termios::LocalModes::ECHO),
            isig: t.local_modes.contains(rustix::termios::LocalModes::ISIG),
        })
    }

    /// Bounded natural wait for the session child to exit.
    pub fn wait_child_exit(
        &mut self,
        budget: Duration,
    ) -> Result<Option<std::process::ExitStatus>, std::io::Error> {
        let outer = Instant::now() + budget.min(self.remaining());
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status));
            }
            let now = Instant::now();
            if now >= outer {
                return Ok(None);
            }
            std::thread::park_timeout(Duration::from_millis(25).min(outer - now));
        }
    }

    /// Non-blocking try_wait for the session child (None if still running).
    pub fn try_wait_child(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    /// Non-waiting kill initiation + bounded reap (root only).
    pub fn terminate_bounded(&mut self, bound: Duration) -> Result<Option<i32>, std::io::Error> {
        let pid = self.child.id();
        if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
        }
        let deadline = Instant::now() + bound;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status.code());
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::park_timeout(Duration::from_millis(25));
        }
    }

    /// Test spawn observation.
    pub fn harness_observation(&self) -> Observation {
        Observation::ProcessSpawned {
            pid: self.child_pid,
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let pid = self.child.id();
        if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
            let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
        }
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::park_timeout(Duration::from_millis(25)),
                Err(_) => return,
            }
        }
    }
}

enum PollOutcome {
    Readable,
    Timeout,
    Hangup,
}

fn set_nonblocking(fd: BorrowedFd<'_>) -> std::io::Result<()> {
    use rustix::fs::{fcntl_getfl, fcntl_setfl};
    let flags = fcntl_getfl(fd).map_err(|e| io_err("fcntl_getfl", e))?;
    fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK).map_err(|e| io_err("fcntl_setfl", e))
}

fn poll_readable(fd: BorrowedFd<'_>, timeout: Duration) -> std::io::Result<PollOutcome> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let mut fds = [PollFd::new(
        &fd,
        PollFlags::IN | PollFlags::HUP | PollFlags::ERR,
    )];
    let ts = Timespec {
        tv_sec: timeout.as_secs() as _,
        tv_nsec: timeout.subsec_nanos() as _,
    };
    let n = poll(&mut fds, Some(&ts)).map_err(|e| io_err("poll", e))?;
    if n == 0 {
        return Ok(PollOutcome::Timeout);
    }
    let revents = fds[0].revents();
    if revents.contains(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
        return Ok(PollOutcome::Hangup);
    }
    if revents.contains(PollFlags::IN) {
        return Ok(PollOutcome::Readable);
    }
    Ok(PollOutcome::Timeout)
}

/// Allow Read on a borrowed master for internal helpers.
#[allow(dead_code)]
fn read_master_slice(fd: BorrowedFd<'_>, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::os::fd::FromRawFd;
    let raw = rustix::io::dup(fd)
        .map_err(|e| io_err("dup_master_read", e))?
        .into_raw_fd();
    let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
    file.read(buf).map_err(|e| io_err("read_master", e))
}
