//! Compat-owned independent Windows ConPTY control harness.
//!
//! Uses CreatePseudoConsole / CreateProcessW directly via windows-sys.
//! Never uses omen-engine's ConPTY (D2-020).
//!
//! D2-022 — synchronous does not mean unbounded: caller/test threads never
//! perform potentially blocking ConPTY `WriteFile` or `ClosePseudoConsole`.
//! A serial input worker owns writes (targeted `CancelSynchronousIo` on
//! timeout); an output drain worker stays live through close; a close worker
//! owns `ClosePseudoConsole`. All caller-facing waits are bounded; no
//! unbounded join.

use crate::windows::observe::{
    WinFileType, WinTranscript, WindowsConPtyShutdownObservation, WindowsConsoleDimensions,
    WindowsWriteObservation, WindowsWriteOutcome,
};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{GetFileType, ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, GetConsoleMode, GetConsoleScreenBufferInfo,
    HPCON, ResizePseudoConsole, SetConsoleMode,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentThreadId, GetExitCodeProcess,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenThread,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, THREAD_TERMINATE, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// Outer bound default for a control scenario.
pub const WIN_SCENARIO_BUDGET: Duration = Duration::from_secs(20);
/// Cleanup bound after explicit terminate / Drop best-effort.
pub const WIN_CLEANUP_BOUND: Duration = Duration::from_secs(3);
/// Second bound after `CancelSynchronousIo` for worker completion.
pub const WIN_WRITE_CANCEL_BOUND: Duration = Duration::from_secs(1);
/// Explicit close-worker completion budget for `shutdown_bounded`.
pub const WIN_CLOSE_BOUND: Duration = Duration::from_secs(3);
/// Worker readiness / stop join bound (still bounded; no bare join).
pub const WIN_WORKER_BOUND: Duration = Duration::from_secs(2);
/// Default caller write budget (min remaining scenario time).
pub const WIN_WRITE_BUDGET: Duration = Duration::from_secs(5);

const ERROR_OPERATION_ABORTED: i32 = 995;
const WIN_POLL: u64 = 20;

fn io_err(phase: &str, err: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("{phase}: {err}"))
}

fn last_os(phase: &str) -> std::io::Error {
    std::io::Error::other(format!("{phase}: {}", std::io::Error::last_os_error()))
}

/// Build a Windows command line from argv (same escaping rules as product).
pub fn build_windows_cmd_line(argv: &[String]) -> String {
    let mut cmd = String::new();
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            cmd.push(' ');
        }
        let needs_quotes =
            arg.is_empty() || arg.contains(' ') || arg.contains('\t') || arg.contains('"');
        if needs_quotes {
            cmd.push('"');
            for c in arg.chars() {
                if c == '"' {
                    cmd.push('\\');
                    cmd.push('"');
                } else {
                    cmd.push(c);
                }
            }
            cmd.push('"');
        } else {
            cmd.push_str(arg);
        }
    }
    cmd
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn lock_transcript(t: &Arc<Mutex<WinTranscript>>) -> std::sync::MutexGuard<'_, WinTranscript> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// Request addressed to the single serial input worker (one active write).
enum InputWorkerRequest {
    Write { id: u64, bytes: Vec<u8> },
    Stop,
}

/// Worker → owner messages (identity, completion, exit).
enum InputWorkerMessage {
    Ready {
        thread_id: u32,
    },
    Complete {
        id: u64,
        written: usize,
        error: Option<i32>,
    },
    Stopped,
}

/// HANDLE is a raw pointer; this newtype makes worker handoff explicit and Send.
#[derive(Clone, Copy)]
struct OwnedWriteHandle(HANDLE);
unsafe impl Send for OwnedWriteHandle {}

impl OwnedWriteHandle {
    /// Method call (not field capture) so closures move the Send newtype.
    fn as_raw(&self) -> HANDLE {
        self.0
    }
}

struct WriteCompletion {
    #[allow(dead_code)]
    id: u64,
    written: usize,
    error: Option<i32>,
}

/// One serial synchronous writer thread. Never a thread pool (D2-022).
///
/// The worker owns the byte buffer for the lifetime of each `WriteFile`.
/// Cancellation targets this exact thread id via `CancelSynchronousIo`.
pub struct WindowsSyncInputWorker {
    request_tx: Sender<InputWorkerRequest>,
    message_rx: Receiver<InputWorkerMessage>,
    thread: Option<JoinHandle<()>>,
    worker_thread_id: Option<u32>,
    next_id: u64,
    active_id: Option<u64>,
    poisoned: bool,
    stopped: bool,
    source: String,
}

impl WindowsSyncInputWorker {
    /// Start a dedicated writer for `write_handle` (handle value only; owner closes after stop).
    pub fn spawn(write_handle: HANDLE, source: &str) -> std::io::Result<Self> {
        if write_handle.is_null() || write_handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::other("input worker: invalid write handle"));
        }
        let (request_tx, request_rx) = mpsc::channel::<InputWorkerRequest>();
        let (message_tx, message_rx) = mpsc::channel::<InputWorkerMessage>();
        let handle = OwnedWriteHandle(write_handle);
        let thread = std::thread::Builder::new()
            .name("compat-win-input".into())
            .spawn(move || {
                let _ = message_tx.send(InputWorkerMessage::Ready {
                    thread_id: unsafe { GetCurrentThreadId() },
                });
                while let Ok(msg) = request_rx.recv() {
                    match msg {
                        InputWorkerRequest::Write { id, bytes } => {
                            let (written, error) = sync_write_all(handle.as_raw(), &bytes);
                            let _ = message_tx.send(InputWorkerMessage::Complete {
                                id,
                                written,
                                error,
                            });
                        }
                        InputWorkerRequest::Stop => break,
                    }
                }
                let _ = message_tx.send(InputWorkerMessage::Stopped);
            })
            .map_err(|e| io_err("spawn_input_worker", e))?;

        let mut worker = Self {
            request_tx,
            message_rx,
            thread: Some(thread),
            worker_thread_id: None,
            next_id: 0,
            active_id: None,
            poisoned: false,
            stopped: false,
            source: source.into(),
        };
        worker.await_ready(Duration::from_secs(1))?;
        Ok(worker)
    }

    fn await_ready(&mut self, bound: Duration) -> std::io::Result<()> {
        match self.message_rx.recv_timeout(bound) {
            Ok(InputWorkerMessage::Ready { thread_id }) => {
                self.worker_thread_id = Some(thread_id);
                Ok(())
            }
            Ok(InputWorkerMessage::Stopped) => {
                Err(std::io::Error::other("input worker exited before ready"))
            }
            Ok(InputWorkerMessage::Complete { .. }) => self.await_ready(bound),
            Err(RecvTimeoutError::Timeout) => {
                Err(std::io::Error::other("input worker ready timeout"))
            }
            Err(RecvTimeoutError::Disconnected) => Err(std::io::Error::other(
                "input worker disconnected before ready",
            )),
        }
    }

    pub fn worker_thread_id(&self) -> Option<u32> {
        self.worker_thread_id
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn active_request_id(&self) -> Option<u64> {
        self.active_id
    }

    /// Request targeted cancellation of this worker's pending synchronous I/O.
    ///
    /// `CancelSynchronousIo` is a request; the caller must still observe a
    /// subsequent completion within a second bound.
    pub fn request_cancel(&self) -> bool {
        let Some(tid) = self.worker_thread_id else {
            return false;
        };
        unsafe {
            let th = OpenThread(THREAD_TERMINATE, 0, tid);
            if th.is_null() {
                return false;
            }
            let ok = CancelSynchronousIo(th);
            CloseHandle(th);
            ok != 0
        }
    }

    fn drain_complete_for(&mut self, id: u64, bound: Duration) -> Option<WriteCompletion> {
        let deadline = Instant::now() + bound;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.message_rx.recv_timeout(remaining) {
                Ok(InputWorkerMessage::Complete {
                    id: cid,
                    written,
                    error,
                }) => {
                    if cid == id {
                        self.active_id = None;
                        return Some(WriteCompletion { id, written, error });
                    }
                    // Serial worker: unexpected id means state corruption — drop it.
                }
                Ok(InputWorkerMessage::Stopped) => {
                    self.stopped = true;
                    self.active_id = None;
                    return None;
                }
                Ok(InputWorkerMessage::Ready { .. }) => {}
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    self.active_id = None;
                    return None;
                }
            }
        }
    }

    /// Bounded caller-facing write: never blocks past `budget` + `cancel_bound`.
    pub fn write_bounded(
        &mut self,
        bytes: &[u8],
        budget: Duration,
        cancel_bound: Duration,
    ) -> std::io::Result<WindowsWriteObservation> {
        if self.poisoned {
            return Err(std::io::Error::other(
                "input writer poisoned; no further writes",
            ));
        }
        if self.stopped {
            return Err(std::io::Error::other(
                "input worker stopped; no further writes",
            ));
        }
        if self.active_id.is_some() {
            return Err(std::io::Error::other(
                "input worker already has an active write (serial only)",
            ));
        }

        self.next_id += 1;
        let id = self.next_id;
        let started = Instant::now();
        self.request_tx
            .send(InputWorkerRequest::Write {
                id,
                bytes: bytes.to_vec(),
            })
            .map_err(|e| io_err("send_write_request", e))?;
        self.active_id = Some(id);

        let mut cancel_requested = false;
        let completion = match self.drain_complete_for(id, budget) {
            Some(c) => Some(c),
            None => {
                cancel_requested = true;
                let _ = self.request_cancel();
                self.drain_complete_for(id, cancel_bound)
            }
        };

        let elapsed = started.elapsed();
        match completion {
            Some(c) => {
                let outcome = match c.error {
                    None if c.written == bytes.len() => WindowsWriteOutcome::Completed,
                    None => WindowsWriteOutcome::Failed,
                    Some(ERROR_OPERATION_ABORTED) => WindowsWriteOutcome::Cancelled,
                    Some(_) => WindowsWriteOutcome::Failed,
                };
                Ok(WindowsWriteObservation {
                    request_id: id,
                    requested_bytes: bytes.len(),
                    written_bytes: c.written,
                    outcome,
                    elapsed,
                    cancel_requested,
                    worker_completed: true,
                    source: self.source.clone(),
                })
            }
            None => {
                self.poisoned = true;
                Ok(WindowsWriteObservation {
                    request_id: id,
                    requested_bytes: bytes.len(),
                    written_bytes: 0,
                    outcome: WindowsWriteOutcome::TimedOut,
                    elapsed,
                    cancel_requested,
                    worker_completed: false,
                    source: self.source.clone(),
                })
            }
        }
    }

    /// Stop the worker and join only after a completion signal (never bare join).
    pub fn stop_bounded(&mut self, bound: Duration) -> bool {
        if self.stopped {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            return true;
        }
        let _ = self.request_tx.send(InputWorkerRequest::Stop);
        let deadline = Instant::now() + bound;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match self.message_rx.recv_timeout(remaining) {
                Ok(InputWorkerMessage::Stopped) => {
                    self.stopped = true;
                    self.active_id = None;
                    if let Some(t) = self.thread.take() {
                        let _ = t.join();
                    }
                    return true;
                }
                Ok(InputWorkerMessage::Complete { id, .. }) => {
                    if self.active_id == Some(id) {
                        self.active_id = None;
                    }
                }
                Ok(InputWorkerMessage::Ready { .. }) => {}
                Err(RecvTimeoutError::Timeout) => return false,
                Err(RecvTimeoutError::Disconnected) => {
                    self.stopped = true;
                    if let Some(t) = self.thread.take() {
                        let _ = t.join();
                    }
                    return true;
                }
            }
        }
    }
}

impl Drop for WindowsSyncInputWorker {
    fn drop(&mut self) {
        // Best-effort: signal stop; join only if worker already completed exit.
        let _ = self.request_tx.send(InputWorkerRequest::Stop);
        if self.stopped {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
            return;
        }
        // Non-blocking: if Stopped is already queued, join; otherwise abandon join.
        if let Ok(InputWorkerMessage::Stopped) =
            self.message_rx.recv_timeout(Duration::from_millis(50))
        {
            self.stopped = true;
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

/// Synchronous WriteFile until complete or error. Buffer owned by this frame
/// for the full syscall lifetime (never dropped by the caller on timeout).
fn sync_write_all(handle: HANDLE, bytes: &[u8]) -> (usize, Option<i32>) {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let mut written: u32 = 0;
        let ok = unsafe {
            WriteFile(
                handle,
                bytes[offset..].as_ptr(),
                (bytes.len() - offset) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            return (offset, Some(err.raw_os_error().unwrap_or(-1)));
        }
        if written == 0 {
            return (offset, Some(-1));
        }
        offset += written as usize;
    }
    (offset, None)
}

struct CloseDone(#[allow(dead_code)] Option<()>);

/// Live Compat-owned ConPTY session with one client process.
pub struct WindowsConPtySession {
    hpcon: HPCON,
    process: HANDLE,
    /// Harness input worker writes here (toward the ConPTY client).
    input_write: HANDLE,
    /// Owned by the output drain worker after spawn (session does not read).
    output_read: HANDLE,
    transcript: Arc<Mutex<WinTranscript>>,
    child_pid: u32,
    started: Instant,
    deadline: Duration,
    rows: u16,
    cols: u16,
    closed: bool,
    stop_accepting_input: Arc<AtomicBool>,
    close_returned: Arc<AtomicBool>,
    input_worker: Option<WindowsSyncInputWorker>,
    input_worker_stopped: bool,
    output_thread: Option<JoinHandle<()>>,
    output_exit_rx: Option<Receiver<()>>,
    output_running: bool,
    close_thread: Option<JoinHandle<()>>,
    close_done_rx: Option<Receiver<CloseDone>>,
    close_started: bool,
    close_timed_out: bool,
    handles_closed: bool,
}

impl WindowsConPtySession {
    /// Spawn `program` as a ConPTY client attached to this session's HPCON.
    pub fn spawn(
        program: impl AsRef<Path>,
        args: &[String],
        cwd: Option<&Path>,
        rows: u16,
        cols: u16,
        deadline: Duration,
    ) -> std::io::Result<Self> {
        if deadline.is_zero() {
            return Err(std::io::Error::other("deadline must be > 0"));
        }

        let mut in_read: HANDLE = std::ptr::null_mut();
        let mut in_write: HANDLE = std::ptr::null_mut();
        let mut out_read: HANDLE = std::ptr::null_mut();
        let mut out_write: HANDLE = std::ptr::null_mut();
        unsafe {
            if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
                return Err(last_os("create_pipe_in"));
            }
            if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                CloseHandle(in_read);
                CloseHandle(in_write);
                return Err(last_os("create_pipe_out"));
            }
        }

        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };
        let mut hpcon: HPCON = 0;
        let res = unsafe { CreatePseudoConsole(size, in_read, out_write, 0, &mut hpcon) };
        unsafe {
            CloseHandle(in_read);
            CloseHandle(out_write);
        }
        if res != 0 {
            unsafe {
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(std::io::Error::other(format!(
                "CreatePseudoConsole failed: {res:#x}"
            )));
        }

        let mut attr_size: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
        }
        let mut attr_buf = vec![0u8; attr_size];
        let attr_list = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) } == 0 {
            unsafe {
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(last_os("InitializeProcThreadAttributeList"));
        }
        if unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpcon as *const std::ffi::c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0
        {
            unsafe {
                DeleteProcThreadAttributeList(attr_list);
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(last_os("UpdateProcThreadAttribute"));
        }

        let mut argv_full: Vec<String> = vec![program.as_ref().to_string_lossy().into_owned()];
        argv_full.extend(args.iter().cloned());
        let cmd_line = build_windows_cmd_line(&argv_full);
        let mut cmd_wide = wide(&cmd_line);

        let mut si_ex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si_ex.StartupInfo.hStdInput = std::ptr::null_mut();
        si_ex.StartupInfo.hStdOutput = std::ptr::null_mut();
        si_ex.StartupInfo.hStdError = std::ptr::null_mut();
        si_ex.lpAttributeList = attr_list;

        let cwd_wide: Vec<u16> = cwd
            .map(|p| {
                p.as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect()
            })
            .unwrap_or_else(|| vec![0]);

        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;
        let cp_ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd_wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                flags,
                std::ptr::null_mut(),
                cwd_wide.as_ptr(),
                &si_ex.StartupInfo,
                &mut pi,
            )
        };
        unsafe {
            DeleteProcThreadAttributeList(attr_list);
        }
        if cp_ok == 0 {
            unsafe {
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(last_os("CreateProcessW"));
        }
        if unsafe { ResumeThread(pi.hThread) } == u32::MAX {
            unsafe {
                TerminateProcess(pi.hProcess, 1);
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(last_os("ResumeThread"));
        }
        unsafe {
            CloseHandle(pi.hThread);
        }

        let transcript = Arc::new(Mutex::new(WinTranscript::default()));
        let stop_accepting_input = Arc::new(AtomicBool::new(false));
        let close_returned = Arc::new(AtomicBool::new(false));

        // Output drain worker owns out_read for the session lifetime.
        let out_handle = OwnedWriteHandle(out_read);
        let out_transcript = Arc::clone(&transcript);
        let out_close_returned = Arc::clone(&close_returned);
        let (out_exit_tx, out_exit_rx) = mpsc::channel::<()>();
        let out_thread = std::thread::Builder::new()
            .name("compat-win-output".into())
            .spawn(move || {
                output_drain_loop(out_handle.as_raw(), &out_transcript, &out_close_returned);
                let _ = out_exit_tx.send(());
                unsafe {
                    let raw = out_handle.as_raw();
                    if !raw.is_null() && raw != INVALID_HANDLE_VALUE {
                        CloseHandle(raw);
                    }
                }
            })
            .map_err(|e| {
                unsafe {
                    ClosePseudoConsole(hpcon);
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                    CloseHandle(pi.hProcess);
                }
                io_err("spawn_output_worker", e)
            })?;

        let input_worker = match WindowsSyncInputWorker::spawn(in_write, "session_input") {
            Ok(w) => w,
            Err(e) => {
                // Best-effort: request close on worker thread; abandon join if slow.
                let (ctx, crx) = mpsc::channel::<CloseDone>();
                let h = hpcon;
                let _ct = std::thread::Builder::new()
                    .name("compat-win-close".into())
                    .spawn(move || {
                        unsafe {
                            ClosePseudoConsole(h);
                        }
                        let _ = ctx.send(CloseDone(Some(())));
                    });
                let _ = crx.recv_timeout(WIN_CLOSE_BOUND);
                unsafe {
                    CloseHandle(in_write);
                    CloseHandle(pi.hProcess);
                }
                let _ = out_exit_rx.recv_timeout(WIN_WORKER_BOUND);
                let _ = out_thread.join();
                return Err(e);
            }
        };

        Ok(Self {
            hpcon,
            process: pi.hProcess,
            input_write: in_write,
            output_read: out_read,
            transcript,
            child_pid: pi.dwProcessId,
            started: Instant::now(),
            deadline,
            rows,
            cols,
            closed: false,
            stop_accepting_input,
            close_returned,
            input_worker: Some(input_worker),
            input_worker_stopped: false,
            output_thread: Some(out_thread),
            output_exit_rx: Some(out_exit_rx),
            output_running: true,
            close_thread: None,
            close_done_rx: None,
            close_started: false,
            close_timed_out: false,
            handles_closed: false,
        })
    }

    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    pub fn process_handle(&self) -> HANDLE {
        self.process
    }

    pub fn transcript(&self) -> std::sync::MutexGuard<'_, WinTranscript> {
        lock_transcript(&self.transcript)
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_sub(self.started.elapsed())
    }

    /// Poll shared transcript (output worker services the pipe). Never reads
    /// the ConPTY pipe on the caller thread.
    pub fn pump_until(
        &mut self,
        predicate: impl Fn(&WinTranscript) -> bool,
        budget: Duration,
    ) -> std::io::Result<WinTranscript> {
        let outer = Instant::now() + budget.min(self.remaining().max(Duration::from_millis(1)));
        loop {
            {
                let t = lock_transcript(&self.transcript);
                if predicate(&t) {
                    return Ok(t.clone());
                }
            }
            if Instant::now() >= outer {
                let mut t = lock_transcript(&self.transcript);
                t.bounded_out = true;
                return Ok(t.clone());
            }
            std::thread::sleep(Duration::from_millis(WIN_POLL));
        }
    }

    /// Write user-like input via the dedicated worker (never on this thread).
    pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<WindowsWriteObservation> {
        if self.closed || self.stop_accepting_input.load(Ordering::SeqCst) {
            return Err(std::io::Error::other(
                "write rejected: session stop-accepting-input",
            ));
        }
        if self.remaining().is_zero() {
            return Err(std::io::Error::other("write after scenario deadline"));
        }
        let budget = self.remaining().min(WIN_WRITE_BUDGET);
        let worker = self
            .input_worker
            .as_mut()
            .ok_or_else(|| std::io::Error::other("input worker missing"))?;
        worker.write_bounded(bytes, budget, WIN_WRITE_CANCEL_BOUND)
    }

    /// Wait for exact-line or substring barrier via shared transcript.
    pub fn wait_for_text(
        &mut self,
        needle: &str,
        budget: Duration,
    ) -> std::io::Result<WinTranscript> {
        {
            let t = lock_transcript(&self.transcript);
            if t.contains(needle) {
                return Ok(t.clone());
            }
        }
        self.pump_until(|t| t.contains(needle), budget)
    }

    /// Resize this session's HPCON (API success ≠ client observation).
    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };
        let res = unsafe { ResizePseudoConsole(self.hpcon, size) };
        if res != 0 {
            return Err(std::io::Error::other(format!(
                "ResizePseudoConsole failed: {res:#x}"
            )));
        }
        self.rows = rows;
        self.cols = cols;
        Ok(())
    }

    /// Bounded wait for process exit; returns raw exit code when observed.
    pub fn wait_exit(&mut self, budget: Duration) -> std::io::Result<Option<u32>> {
        if self.process.is_null() {
            return Ok(None);
        }
        let ms = budget
            .min(self.remaining().max(Duration::from_millis(1)))
            .as_millis()
            .min(u32::MAX as u128) as u32;
        let w = unsafe { WaitForSingleObject(self.process, ms) };
        if w == WAIT_OBJECT_0 {
            let mut code: u32 = 0;
            unsafe {
                GetExitCodeProcess(self.process, &mut code);
            }
            return Ok(Some(code));
        }
        Ok(None)
    }

    /// Non-blocking exit poll.
    pub fn try_exit(&self) -> std::io::Result<Option<u32>> {
        if self.process.is_null() {
            return Ok(None);
        }
        let w = unsafe { WaitForSingleObject(self.process, 0) };
        if w == WAIT_OBJECT_0 {
            let mut code: u32 = 0;
            unsafe {
                GetExitCodeProcess(self.process, &mut code);
            }
            return Ok(Some(code));
        }
        Ok(None)
    }

    pub fn is_alive(&self) -> bool {
        self.try_exit().ok().flatten().is_none()
    }

    /// Non-waiting terminate + bounded reap.
    pub fn terminate_bounded(&mut self, bound: Duration) -> std::io::Result<Option<u32>> {
        if !self.process.is_null() {
            unsafe {
                TerminateProcess(self.process, 1);
            }
        }
        self.wait_exit(bound)
    }

    fn begin_close_locked(&mut self) {
        if self.close_started {
            return;
        }
        self.close_started = true;
        let hpcon = self.hpcon;
        let close_returned = Arc::clone(&self.close_returned);
        let (done_tx, done_rx) = mpsc::channel::<CloseDone>();
        let thread = std::thread::Builder::new()
            .name("compat-win-close".into())
            .spawn(move || {
                unsafe {
                    ClosePseudoConsole(hpcon);
                }
                close_returned.store(true, Ordering::SeqCst);
                let _ = done_tx.send(CloseDone(Some(())));
            })
            .expect("spawn close worker");
        self.close_thread = Some(thread);
        self.close_done_rx = Some(done_rx);
    }

    fn wait_close(&mut self, bound: Duration) -> bool {
        let Some(rx) = self.close_done_rx.as_ref() else {
            return !self.close_started;
        };
        match rx.recv_timeout(bound) {
            Ok(CloseDone(_)) => {
                if let Some(t) = self.close_thread.take() {
                    let _ = t.join();
                }
                self.close_done_rx = None;
                self.close_returned.store(true, Ordering::SeqCst);
                true
            }
            Err(RecvTimeoutError::Timeout) => {
                self.close_timed_out = true;
                false
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.close_returned.store(true, Ordering::SeqCst);
                if let Some(t) = self.close_thread.take() {
                    let _ = t.join();
                }
                self.close_done_rx = None;
                true
            }
        }
    }

    fn wait_output_worker(&mut self, bound: Duration) -> bool {
        if !self.output_running {
            return true;
        }
        let Some(rx) = self.output_exit_rx.take() else {
            self.output_running = false;
            return true;
        };
        match rx.recv_timeout(bound) {
            Ok(()) => {
                if let Some(t) = self.output_thread.take() {
                    let _ = t.join();
                }
                self.output_running = false;
                true
            }
            Err(RecvTimeoutError::Timeout) => {
                // Keep receiver for a later attempt; do not unbounded-join.
                self.output_exit_rx = Some(rx);
                false
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(t) = self.output_thread.take() {
                    let _ = t.join();
                }
                self.output_running = false;
                true
            }
        }
    }

    fn close_owned_handles(&mut self) {
        if self.handles_closed {
            return;
        }
        // input_write: only after input worker stopped (single owner close).
        if !self.input_write.is_null() && self.input_write != INVALID_HANDLE_VALUE {
            let worker_stopped = self.input_worker_stopped;
            if worker_stopped {
                unsafe {
                    CloseHandle(self.input_write);
                }
                self.input_write = std::ptr::null_mut();
            }
        }
        // process: single close once wait path is done.
        if !self.process.is_null() && self.process != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.process);
            }
            self.process = std::ptr::null_mut();
        }
        // output_read: owned/closed by output worker when it exits.
        if !self.output_running {
            self.output_read = std::ptr::null_mut();
        }
        // Mark only when every owned close path completed (no leak claim on partial).
        if self.input_worker_stopped && !self.output_running {
            self.handles_closed = true;
        }
    }

    /// Explicit bounded teardown (D2-022). Returns typed evidence.
    ///
    /// Stages: stop accepting input → cancel active write → terminate/wait
    /// client → close worker (`ClosePseudoConsole` off this thread) → output
    /// drain continues → close completion observed → workers joined only
    /// after exit signal → single-close handles.
    pub fn shutdown_bounded(&mut self, budget: Duration) -> WindowsConPtyShutdownObservation {
        if self.closed {
            return WindowsConPtyShutdownObservation::already_closed("session");
        }
        let started = Instant::now();
        let budget = budget.max(Duration::from_millis(1));
        let mut bounded_out = false;
        let mut active_write_cancelled = false;

        // STOP_ACCEPTING_INPUT
        self.stop_accepting_input.store(true, Ordering::SeqCst);

        // CANCEL_ACTIVE_WRITE (if any) — targeted at the exact worker thread.
        if let Some(worker) = self.input_worker.as_ref()
            && worker.active_request_id().is_some()
        {
            active_write_cancelled = worker.request_cancel();
            if let Some(w) = self.input_worker.as_mut() {
                let id = w.active_request_id().unwrap_or(0);
                let _ = w.drain_complete_for(id, WIN_WRITE_CANCEL_BOUND);
                active_write_cancelled = true;
            }
        }

        // TERMINATE / WAIT CLIENT
        let client_exit_observed = if !self.process.is_null() {
            let alive = self.is_alive();
            if alive {
                unsafe {
                    TerminateProcess(self.process, 1);
                }
            }
            let term_bound = budget.saturating_sub(started.elapsed());
            let _ = self.wait_exit(term_bound.min(Duration::from_secs(2)));
            self.try_exit().ok().flatten().is_some()
        } else {
            true
        };

        // INPUT WORKER STOP (join only after Stopped signal).
        let input_worker_stopped = if let Some(w) = self.input_worker.as_mut() {
            let bound = budget
                .saturating_sub(started.elapsed())
                .max(Duration::from_millis(1));
            w.stop_bounded(bound.min(WIN_WORKER_BOUND))
        } else {
            true
        };
        self.input_worker_stopped = input_worker_stopped;
        if !input_worker_stopped {
            bounded_out = true;
        }

        // BEGIN_CONPTY_CLOSE on dedicated worker; output drain continues.
        self.begin_close_locked();
        let close_budget = budget
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(1));
        let close_returned = self.wait_close(close_budget.min(WIN_CLOSE_BOUND));
        if !close_returned {
            bounded_out = true;
        }

        // OUTPUT DRAIN: worker exits on pipe break or close-returned + idle.
        let out_budget = budget
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(1));
        let output_worker_stopped = self.wait_output_worker(out_budget.min(WIN_WORKER_BOUND));
        if !output_worker_stopped {
            bounded_out = true;
        }

        let output_pipe_broken = {
            let t = lock_transcript(&self.transcript);
            t.eof_or_hangup
        };

        // WORKERS EXIT → HANDLES CLOSED ONCE
        self.close_owned_handles();
        if !input_worker_stopped || !output_worker_stopped || !close_returned {
            // Partial ownership: do not claim full closed state for reuse.
            bounded_out = true;
        } else {
            self.closed = true;
        }

        WindowsConPtyShutdownObservation {
            input_worker_stopped,
            active_write_cancelled,
            client_exit_observed,
            close_started: self.close_started,
            close_returned,
            close_timed_out: self.close_timed_out,
            output_pipe_broken,
            output_worker_stopped,
            handles_closed_once: self.handles_closed,
            elapsed: started.elapsed(),
            bounded_out: bounded_out || started.elapsed() > budget,
            source: "session.shutdown_bounded".into(),
        }
    }

    /// Back-compat alias used by existing tests: bounded shutdown, no return.
    pub fn close_handles(&mut self) {
        let _ = self.shutdown_bounded(WIN_CLEANUP_BOUND.max(Duration::from_secs(2)));
    }
}

impl Drop for WindowsConPtySession {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // Best-effort bounded reap only. Never unbounded ClosePseudoConsole
        // or unbounded worker join on Drop (D2-022 §26).
        let _ = self.shutdown_bounded(WIN_CLEANUP_BOUND);
        // Partial failure: abandon remaining handles/threads without a second
        // unbounded attempt; process exit reclaims abandoned OS resources.
    }
}

/// Output drain worker: PeekNamedPipe + ReadFile only when data exists.
/// Remains active while `ClosePseudoConsole` runs on the close worker.
fn output_drain_loop(
    handle: HANDLE,
    transcript: &Arc<Mutex<WinTranscript>>,
    close_returned: &AtomicBool,
) {
    let mut idle_after_close = 0u32;
    loop {
        let mut available: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            let code = err.raw_os_error().unwrap_or(0);
            if code == 109 || code == 232 || code == 233 {
                lock_transcript(transcript).eof_or_hangup = true;
                return;
            }
            // Transient peek failure: poll until close returns or budget idle.
            if close_returned.load(Ordering::SeqCst) {
                idle_after_close += 1;
                if idle_after_close > 50 {
                    lock_transcript(transcript).eof_or_hangup = true;
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(WIN_POLL));
            continue;
        }
        if available == 0 {
            if close_returned.load(Ordering::SeqCst) {
                idle_after_close += 1;
                // After close returns, drain remaining then exit cleanly.
                if idle_after_close > 25 {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(WIN_POLL));
            continue;
        }
        idle_after_close = 0;
        let mut buf = [0u8; 4096];
        let mut read_n: u32 = 0;
        let r = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr(),
                (available as usize).min(buf.len()) as u32,
                &mut read_n,
                std::ptr::null_mut(),
            )
        };
        if r == 0 {
            lock_transcript(transcript).eof_or_hangup = true;
            return;
        }
        if read_n > 0 {
            lock_transcript(transcript).push(&buf[..read_n as usize]);
        }
    }
}

/// Deterministic hostile backpressure control (no ConPTY involved).
///
/// Compat-owned synchronous pipe: reader handle stays alive but is never
/// drained. The input worker blocks in `WriteFile`; the caller deadline
/// expires; `CancelSynchronousIo` targets that worker; completion is observed
/// within the cancellation bound. Proves the worker abstraction independently
/// of ConPTY behaviour (D2-022 §12).
pub fn control_blocked_write(
    write_budget: Duration,
    cancel_bound: Duration,
) -> std::io::Result<WindowsWriteObservation> {
    let mut in_read: HANDLE = std::ptr::null_mut();
    let mut in_write: HANDLE = std::ptr::null_mut();
    unsafe {
        if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
            return Err(last_os("blocked_write_create_pipe"));
        }
    }
    // Intentionally do not drain `in_read`. Keep it alive for the write lifetime.
    let mut worker = WindowsSyncInputWorker::spawn(in_write, "control_blocked_write")?;
    // Large enough to fill the anonymous pipe buffer and block.
    let payload = vec![b'X'; 1024 * 1024];
    let obs = worker.write_bounded(&payload, write_budget, cancel_bound);
    let stopped = worker.stop_bounded(cancel_bound.max(Duration::from_secs(1)));
    // Single-close: reader always; writer only after worker proven stopped.
    unsafe {
        if !in_read.is_null() {
            CloseHandle(in_read);
        }
        if stopped && !in_write.is_null() {
            CloseHandle(in_write);
        }
    }
    // Worker does not own handles; Drop joins only if already stopped.
    drop(worker);
    obs
}

/// Probe `ReleasePseudoConsole` availability without requiring it (D2-022 §23).
pub fn release_pseudoconsole_available() -> bool {
    use std::sync::OnceLock;
    static AVAIL: OnceLock<bool> = OnceLock::new();
    *AVAIL.get_or_init(|| unsafe {
        let k32 = GetModuleHandleW(wide("kernel32").as_ptr());
        if k32.is_null() {
            return false;
        }
        let sym = GetProcAddress(k32, c"ReleasePseudoConsole".as_ptr().cast::<u8>());
        sym.is_some()
    })
}

/// Observe GetFileType on a raw handle (None if invalid/null).
///
/// # Safety
/// `handle` must be a valid HANDLE or null.
pub unsafe fn observe_file_type(handle: HANDLE) -> Option<WinFileType> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let t = unsafe { GetFileType(handle) };
    if t == 0 {
        return None;
    }
    Some(WinFileType::from_raw(t))
}

/// Observe GetConsoleMode on a handle (None if not a console handle).
///
/// # Safety
/// `handle` must be a valid HANDLE or null.
pub unsafe fn observe_console_mode(handle: HANDLE) -> Option<u32> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut mode: u32 = 0;
    let ok = unsafe { GetConsoleMode(handle, &mut mode) };
    if ok == 0 { None } else { Some(mode) }
}

/// Set console mode (for dirty fixture / calibration).
///
/// # Safety
/// `handle` must be a valid console HANDLE.
pub unsafe fn set_console_mode(handle: HANDLE, mode: u32) -> std::io::Result<()> {
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(last_os("SetConsoleMode"));
    }
    Ok(())
}

/// Observe console dimensions from a console handle via GetConsoleScreenBufferInfo.
///
/// # Safety
/// `handle` must be a valid HANDLE or null.
pub unsafe fn observe_console_dimensions(handle: HANDLE) -> WindowsConsoleDimensions {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return WindowsConsoleDimensions::unavailable("null handle");
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ScreenBufferInfo {
        dw_size: Coord,
        dw_cursor_position: Coord,
        w_attributes: u16,
        sr_window: SmallRect,
        dw_maximum_window_size: Coord,
    }
    let mut info: ScreenBufferInfo = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetConsoleScreenBufferInfo(handle, &mut info as *mut _ as *mut _) };
    if ok == 0 {
        return WindowsConsoleDimensions::unavailable(format!(
            "GetConsoleScreenBufferInfo failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let rows = (info.sr_window.bottom - info.sr_window.top + 1) as u16;
    let cols = (info.sr_window.right - info.sr_window.left + 1) as u16;
    WindowsConsoleDimensions {
        rows: Some(rows),
        cols: Some(cols),
        available: true,
        source: "GetConsoleScreenBufferInfo".into(),
    }
}

/// Format path for gremlin-like binary resolution helpers used by tests.
pub fn workspace_root_from_manifest(manifest: &str) -> PathBuf {
    Path::new(manifest)
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}
