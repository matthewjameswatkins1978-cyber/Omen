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
//!
//! D2-023 — the failure path is still the path: every post-HPCON construction
//! failure runs through one construction guard and the same bounded close
//! authority as normal teardown. A timed wait never grants permission to
//! join, and close initiation revokes the session's usable pseudoconsole
//! token immediately.

use crate::windows::observe::{
    WinFileType, WinTranscript, WindowsConPtyShutdownObservation, WindowsConsoleDimensions,
    WindowsConstructionCleanupObservation, WindowsWriteObservation, WindowsWriteOutcome,
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
/// Absolute wall-clock bound for one construction-failure cleanup (D2-023).
pub const WIN_CONSTRUCTION_CLEANUP_BOUND: Duration = Duration::from_secs(10);
/// Bounded wait for a terminated construction client to signal exit.
pub const WIN_CLIENT_EXIT_BOUND: Duration = Duration::from_secs(2);

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

/// Deterministic construction fault injection for Compat's own harness
/// (D2-023 §19).
///
/// This is a test-only seam: it is driven by an explicit argument, never by
/// an environment variable or global state, and no naturally failing Win32
/// call is depended upon. Real construction always passes `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructionFault {
    /// Immediately after the pseudoconsole is created (no client yet).
    AfterPseudoConsole,
    /// After the process-thread attribute list is fully wired (no client yet).
    AfterAttributeSetup,
    /// After the client process is created, before it is resumed.
    AfterProcessCreate,
    /// Immediately before the suspended client is resumed.
    BeforeResume,
    /// When the normal output drain worker would be spawned (client running).
    OutputWorkerSpawn,
    /// When the input worker would be spawned (output drain already live).
    InputWorkerSpawn,
}

impl ConstructionFault {
    pub fn stable_id(&self) -> &'static str {
        match self {
            ConstructionFault::AfterPseudoConsole => "after_pseudoconsole",
            ConstructionFault::AfterAttributeSetup => "after_attribute_setup",
            ConstructionFault::AfterProcessCreate => "after_process_create",
            ConstructionFault::BeforeResume => "before_resume",
            ConstructionFault::OutputWorkerSpawn => "output_worker_spawn",
            ConstructionFault::InputWorkerSpawn => "input_worker_spawn",
        }
    }
}

/// Outcome of a join attempted under join discipline (D2-023 §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedJoinOutcome {
    /// An exit signal was received, or the channel disconnected in a way that
    /// mechanically proves the sender thread finished.
    pub exit_observed: bool,
    /// A join was actually performed by this call.
    pub joined: bool,
    /// The bound expired before any exit evidence existed.
    pub timed_out: bool,
    pub elapsed: Duration,
}

/// Join discipline primitive: join ONLY after observed exit evidence.
///
/// A timeout alone is never permission to join (D2-023 §12). On timeout the
/// handle is handed back untouched so the caller retains ownership and the
/// thread is detached only when the caller explicitly gives up on it.
pub fn join_after_exit_signal(
    exit_rx: &Receiver<()>,
    bound: Duration,
    thread: &mut Option<JoinHandle<()>>,
) -> BoundedJoinOutcome {
    let started = Instant::now();
    match exit_rx.recv_timeout(bound) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => {
            // Disconnected proves the sender was dropped by the worker
            // closure itself, i.e. the thread function returned.
            let joined = match thread.take() {
                Some(handle) => handle.join().is_ok(),
                None => false,
            };
            BoundedJoinOutcome {
                exit_observed: true,
                joined,
                timed_out: false,
                elapsed: started.elapsed(),
            }
        }
        Err(RecvTimeoutError::Timeout) => BoundedJoinOutcome {
            exit_observed: false,
            joined: false,
            timed_out: true,
            elapsed: started.elapsed(),
        },
    }
}

/// Worker → owner messages for the dedicated close worker.
enum CloseWorkerMessage {
    Started { thread_id: u32 },
    Done,
}

/// The single bounded close authority for every pseudoconsole (D2-023 §25).
///
/// This type is the only place in Compat that invokes the ConPTY close API.
/// It reports its own thread identity before closing so callers can prove the
/// close did not run on the spawning test thread, and it joins only after the
/// worker reports completion or the channel disconnects.
struct CloseWorkerHandle {
    thread: Option<JoinHandle<()>>,
    rx: Receiver<CloseWorkerMessage>,
    close_returned: Arc<AtomicBool>,
    thread_id: Option<u32>,
    timed_out: bool,
}

impl CloseWorkerHandle {
    /// Launch the close worker. `orphan_out_read` is an output read handle no
    /// worker owns (construction failure with no drain worker); the worker
    /// closes it exactly once, after the close returns.
    fn spawn(
        hpcon: HPCON,
        orphan_out_read: Option<HANDLE>,
        close_returned: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        let orphan = orphan_out_read.map(OwnedWriteHandle);
        let worker_close_returned = Arc::clone(&close_returned);
        let (tx, rx) = mpsc::channel::<CloseWorkerMessage>();
        let thread = std::thread::Builder::new()
            .name("compat-win-close".into())
            .spawn(move || {
                let _ = tx.send(CloseWorkerMessage::Started {
                    thread_id: unsafe { GetCurrentThreadId() },
                });
                unsafe {
                    ClosePseudoConsole(hpcon);
                }
                worker_close_returned.store(true, Ordering::SeqCst);
                if let Some(handle) = orphan {
                    let raw = handle.as_raw();
                    if !raw.is_null() && raw != INVALID_HANDLE_VALUE {
                        unsafe {
                            CloseHandle(raw);
                        }
                    }
                }
                let _ = tx.send(CloseWorkerMessage::Done);
            })
            .map_err(|e| io_err("spawn_close_worker", e))?;
        Ok(Self {
            thread: Some(thread),
            rx,
            close_returned,
            thread_id: None,
            timed_out: false,
        })
    }

    fn thread_id(&self) -> Option<u32> {
        self.thread_id
    }

    /// Wait boundedly for close completion. Joins only after `Done` or a
    /// channel disconnect; a timeout detaches the worker instead.
    ///
    /// Returns whether the close call was observed to return.
    fn wait(&mut self, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.timed_out = true;
                return false;
            }
            let message = self.rx.recv_timeout(remaining);
            match message {
                Ok(CloseWorkerMessage::Started { thread_id }) => {
                    self.thread_id = Some(thread_id);
                }
                Ok(CloseWorkerMessage::Done) => {
                    if let Some(handle) = self.thread.take() {
                        let _ = handle.join();
                    }
                    return true;
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.timed_out = true;
                    return false;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // The worker closure ended. It sets the shared flag before
                    // reporting `Done`, so an abnormal exit must not be
                    // reported as a completed close.
                    let returned = self.close_returned.load(Ordering::SeqCst);
                    if let Some(handle) = self.thread.take() {
                        let _ = handle.join();
                    }
                    return returned;
                }
            }
        }
    }
}

/// Construction step failure: which phase failed and why.
struct ConstructionStepError {
    stage: &'static str,
    error: std::io::Error,
}

/// RAII owner for the process-thread attribute list: exactly one delete.
struct AttributeListGuard {
    buf: Vec<u8>,
    initialized: bool,
}

impl AttributeListGuard {
    fn new(size: usize) -> Self {
        Self {
            buf: vec![0u8; size],
            initialized: false,
        }
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST
    }

    fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    fn delete_now(&mut self) {
        if self.initialized {
            unsafe {
                DeleteProcThreadAttributeList(self.buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST);
            }
            self.initialized = false;
        }
    }
}

impl Drop for AttributeListGuard {
    fn drop(&mut self) {
        self.delete_now();
    }
}

/// Output drain worker for `out_read`: owns and closes the handle on exit.
///
/// On spawn failure the handle is *not* consumed — the caller retains
/// ownership and must account for it.
fn spawn_output_drain(
    out_read: HANDLE,
    transcript: &Arc<Mutex<WinTranscript>>,
    close_returned: &Arc<AtomicBool>,
) -> std::io::Result<(JoinHandle<()>, Receiver<()>)> {
    let handle = OwnedWriteHandle(out_read);
    let drain_transcript = Arc::clone(transcript);
    let drain_close_returned = Arc::clone(close_returned);
    let (exit_tx, exit_rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("compat-win-output".into())
        .spawn(move || {
            output_drain_loop(handle.as_raw(), &drain_transcript, &drain_close_returned);
            let _ = exit_tx.send(());
            unsafe {
                let raw = handle.as_raw();
                if !raw.is_null() && raw != INVALID_HANDLE_VALUE {
                    CloseHandle(raw);
                }
            }
        })
        .map_err(|e| io_err("spawn_output_worker", e))?;
    Ok((thread, exit_rx))
}

/// Single cleanup authority for everything that exists once the
/// pseudoconsole has been created (D2-023 §5).
///
/// Every post-HPCON construction failure hands this guard to
/// [`ConPtyConstructionGuard::cleanup_bounded`]; a successful construction
/// transfers it into the session. There is no other cleanup path, so no
/// spawn error branch can call the ConPTY close API itself.
struct ConPtyConstructionGuard {
    hpcon: Option<HPCON>,
    input_write: Option<HANDLE>,
    /// `Some` only while this guard still owns the output read handle.
    out_read: Option<HANDLE>,
    process: Option<HANDLE>,
    primary_thread: Option<HANDLE>,
    child_pid: u32,
    input_worker: Option<WindowsSyncInputWorker>,
    output_thread: Option<JoinHandle<()>>,
    output_exit_rx: Option<Receiver<()>>,
    output_worker_spawned: bool,
    transcript: Arc<Mutex<WinTranscript>>,
    stop_accepting_input: Arc<AtomicBool>,
    close_returned: Arc<AtomicBool>,
    caller_thread_id: u32,
}

impl ConPtyConstructionGuard {
    fn new(hpcon: HPCON, input_write: HANDLE, out_read: HANDLE, caller_thread_id: u32) -> Self {
        Self {
            hpcon: Some(hpcon),
            input_write: Some(input_write),
            out_read: Some(out_read),
            process: None,
            primary_thread: None,
            child_pid: 0,
            input_worker: None,
            output_thread: None,
            output_exit_rx: None,
            output_worker_spawned: false,
            transcript: Arc::new(Mutex::new(WinTranscript::default())),
            stop_accepting_input: Arc::new(AtomicBool::new(false)),
            close_returned: Arc::new(AtomicBool::new(false)),
            caller_thread_id,
        }
    }

    fn adopt_client(&mut self, process: HANDLE, primary_thread: HANDLE, child_pid: u32) {
        self.process = Some(process);
        self.primary_thread = Some(primary_thread);
        self.child_pid = child_pid;
    }

    /// Single close of the primary thread handle after a successful resume.
    fn release_primary_thread(&mut self) {
        if let Some(handle) = self.primary_thread.take()
            && !handle.is_null()
            && handle != INVALID_HANDLE_VALUE
        {
            unsafe {
                CloseHandle(handle);
            }
        }
    }

    /// The output drain worker now owns the output read handle.
    fn adopt_output_worker(&mut self, thread: JoinHandle<()>, exit_rx: Receiver<()>) {
        self.out_read = None;
        self.output_thread = Some(thread);
        self.output_exit_rx = Some(exit_rx);
        self.output_worker_spawned = true;
    }

    /// Transfer the fully built construction into the live session.
    ///
    /// Ownership moves once: the session becomes the sole holder of the
    /// pseudoconsole, client process, workers, and shared signals.
    fn into_session(self, deadline: Duration, rows: u16, cols: u16) -> WindowsConPtySession {
        let ConPtyConstructionGuard {
            hpcon,
            input_write,
            out_read,
            process,
            primary_thread: _,
            child_pid,
            input_worker,
            output_thread,
            output_exit_rx,
            output_worker_spawned,
            transcript,
            stop_accepting_input,
            close_returned,
            caller_thread_id,
        } = self;
        debug_assert!(
            hpcon.is_some(),
            "construction succeeded with a pseudoconsole"
        );
        debug_assert!(out_read.is_none(), "output read handle must have an owner");
        debug_assert!(process.is_some(), "construction succeeded with a client");
        WindowsConPtySession {
            hpcon,
            process: process.unwrap_or(std::ptr::null_mut()),
            input_write: input_write.unwrap_or(std::ptr::null_mut()),
            transcript,
            child_pid,
            started: Instant::now(),
            deadline,
            rows,
            cols,
            closed: false,
            stop_accepting_input,
            close_returned,
            input_worker,
            input_worker_stopped: false,
            output_thread,
            output_exit_rx,
            output_running: output_worker_spawned,
            close_worker: None,
            close_started: false,
            close_timed_out: false,
            close_thread_id: None,
            caller_thread_id,
            handles_closed: false,
            resize_attempts: 0,
            resize_api_calls: 0,
        }
    }

    /// Bounded cleanup for one construction failure (D2-023 §7).
    ///
    /// Order: stop accepting work → terminate an attached client → stop an
    /// input worker → establish output drainage if a client may have produced
    /// output → move the pseudoconsole to the close worker → wait boundedly →
    /// join workers only after exit evidence → close ordinary handles exactly
    /// once → report what was and was not completed.
    fn cleanup_bounded(mut self, failure_stage: &str) -> WindowsConstructionCleanupObservation {
        let started = Instant::now();
        let deadline = started + WIN_CONSTRUCTION_CLEANUP_BOUND;
        let mut bounded_out = false;
        let cap = |limit: Duration| -> Duration {
            deadline
                .saturating_duration_since(Instant::now())
                .min(limit)
        };

        // 1. STOP_ACCEPTING_WORK
        self.stop_accepting_input.store(true, Ordering::SeqCst);

        // 2. TERMINATE / WAIT CLIENT (including a never-resumed child)
        let client_existed = self.process.is_some();
        let mut client_exit_observed = !client_existed;
        if let Some(process) = self.process {
            unsafe {
                TerminateProcess(process, 1);
            }
            let wait_ms = cap(WIN_CLIENT_EXIT_BOUND)
                .max(Duration::from_millis(250))
                .as_millis()
                .min(u32::MAX as u128) as u32;
            let observed = unsafe { WaitForSingleObject(process, wait_ms) } == WAIT_OBJECT_0;
            client_exit_observed = observed;
            if !observed {
                bounded_out = true;
            }
        }

        // 3. STOP INPUT WORKER (join only after an observed exit signal)
        let input_worker_stopped = match self.input_worker.as_mut() {
            Some(worker) => {
                let bound = cap(WIN_WORKER_BOUND).max(Duration::from_millis(50));
                let stopped = worker.stop_bounded(bound);
                if !stopped {
                    bounded_out = true;
                }
                stopped
            }
            None => true,
        };

        // 4. PRESERVE / ESTABLISH OUTPUT DRAINAGE for an attached client
        if !self.output_worker_spawned
            && client_existed
            && let Some(handle) = self.out_read.take()
        {
            match spawn_output_drain(handle, &self.transcript, &self.close_returned) {
                Ok((thread, exit_rx)) => {
                    self.output_thread = Some(thread);
                    self.output_exit_rx = Some(exit_rx);
                    self.output_worker_spawned = true;
                }
                Err(_) => {
                    // No drain could be established: keep the handle and
                    // record the gap rather than pretending it is fine.
                    self.out_read = Some(handle);
                    bounded_out = true;
                }
            }
        }

        // 5 + 6. MOVE HPCON TO THE CLOSE WORKER, then wait boundedly
        let mut close_started = false;
        let mut close_returned = false;
        let mut close_thread_id = None;
        if let Some(hpcon) = self.hpcon.take() {
            let orphan = self.out_read;
            match CloseWorkerHandle::spawn(hpcon, orphan, Arc::clone(&self.close_returned)) {
                Ok(mut close_worker) => {
                    self.out_read = None;
                    close_started = true;
                    let bound = cap(WIN_CLOSE_BOUND).max(Duration::from_millis(50));
                    close_returned = close_worker.wait(bound);
                    close_thread_id = close_worker.thread_id();
                    if !close_returned {
                        bounded_out = true;
                    }
                }
                Err(_) => {
                    // The caller thread must never close the pseudoconsole.
                    // Hand the token back so it can still be given away later.
                    self.hpcon = Some(hpcon);
                    bounded_out = true;
                }
            }
        }

        // 7. JOIN WORKERS ONLY AFTER EXIT EVIDENCE
        let mut output_worker_exit_observed: Option<bool> = None;
        if self.output_worker_spawned
            && let Some(exit_rx) = self.output_exit_rx.take()
        {
            let bound = cap(WIN_WORKER_BOUND).max(Duration::from_millis(50));
            let outcome = join_after_exit_signal(&exit_rx, bound, &mut self.output_thread);
            output_worker_exit_observed = Some(outcome.exit_observed);
            if !outcome.exit_observed {
                bounded_out = true;
            }
        }

        // 8. CLOSE ORDINARY HANDLES EXACTLY ONCE
        let mut input_write_closed = true;
        if input_worker_stopped {
            if let Some(handle) = self.input_write.take()
                && !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
            {
                unsafe {
                    CloseHandle(handle);
                }
            }
        } else {
            input_write_closed = false;
        }
        if let Some(handle) = self.primary_thread.take()
            && !handle.is_null()
            && handle != INVALID_HANDLE_VALUE
        {
            unsafe {
                CloseHandle(handle);
            }
        }
        if let Some(handle) = self.process.take()
            && !handle.is_null()
            && handle != INVALID_HANDLE_VALUE
        {
            unsafe {
                CloseHandle(handle);
            }
        }

        // Output read ownership: a drain worker closed it when it exited, or
        // the close worker closes it after the close returns. If neither
        // happened, it is still owned here and must not be claimed closed.
        let out_read_accounted = if self.output_worker_spawned {
            output_worker_exit_observed == Some(true)
        } else if close_started {
            close_returned
        } else {
            self.out_read.is_none()
        };

        let handles_closed_once = input_write_closed && out_read_accounted;
        let bounded_out = bounded_out || started.elapsed() > WIN_CONSTRUCTION_CLEANUP_BOUND;

        WindowsConstructionCleanupObservation {
            failure_stage: failure_stage.into(),
            hpcon_created: true,
            client_existed,
            client_exit_observed,
            output_drain_established: self.output_worker_spawned,
            close_started,
            close_returned,
            close_timed_out: close_started && !close_returned,
            caller_thread_id: self.caller_thread_id,
            close_thread_id,
            output_worker_exit_observed,
            handles_closed_once,
            bounded_out,
            elapsed: started.elapsed(),
            source: "conpty_construction.cleanup_bounded".into(),
        }
    }
}

/// A construction failure with its bounded cleanup record attached.
#[derive(Debug)]
pub struct ConPtyConstructionFailure {
    pub error: std::io::Error,
    pub cleanup: WindowsConstructionCleanupObservation,
}

impl std::fmt::Display for ConPtyConstructionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {} [hpcon_created={} client_existed={} close_started={} \
             close_returned={} close_thread={} caller_thread={} handles_closed={} \
             bounded_out={} elapsed_ms={}]",
            self.error,
            self.cleanup.failure_stage,
            self.cleanup.hpcon_created,
            self.cleanup.client_existed,
            self.cleanup.close_started,
            self.cleanup.close_returned,
            self.cleanup
                .close_thread_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unobserved".into()),
            self.cleanup.caller_thread_id,
            self.cleanup.handles_closed_once,
            self.cleanup.bounded_out,
            self.cleanup.elapsed.as_millis(),
        )
    }
}

impl From<ConPtyConstructionFailure> for std::io::Error {
    fn from(failure: ConPtyConstructionFailure) -> Self {
        std::io::Error::other(failure.to_string())
    }
}

fn pre_hpcon_failure(
    stage: &str,
    error: std::io::Error,
    caller_thread_id: u32,
    elapsed: Duration,
    handles_closed_once: bool,
) -> ConPtyConstructionFailure {
    ConPtyConstructionFailure {
        error,
        cleanup: WindowsConstructionCleanupObservation::pre_hpcon(
            stage,
            caller_thread_id,
            elapsed,
            handles_closed_once,
        ),
    }
}

/// Live Compat-owned ConPTY session with one client process.
pub struct WindowsConPtySession {
    /// Live pseudoconsole ownership. `None` from the moment close is
    /// initiated: the close worker is then the sole semantic owner and no
    /// operation can reach a stale token (D2-023 §15).
    hpcon: Option<HPCON>,
    process: HANDLE,
    /// Harness input worker writes here (toward the ConPTY client).
    input_write: HANDLE,
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
    close_worker: Option<CloseWorkerHandle>,
    close_started: bool,
    close_timed_out: bool,
    close_thread_id: Option<u32>,
    caller_thread_id: u32,
    handles_closed: bool,
    resize_attempts: u32,
    resize_api_calls: u32,
}

/// Build everything that must exist after the pseudoconsole is created.
///
/// Every post-HPCON failure funnels through this function's `Err` return, and
/// the single caller runs the construction guard cleanup. No error branch can
/// bypass it, so no error branch can close the pseudoconsole itself (D2-023).
fn build_client(
    guard: &mut ConPtyConstructionGuard,
    fault: Option<ConstructionFault>,
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
) -> Result<(), ConstructionStepError> {
    let injected = |fault: ConstructionFault| ConstructionStepError {
        stage: fault.stable_id(),
        error: std::io::Error::other(format!(
            "injected construction fault: {}",
            fault.stable_id()
        )),
    };

    if fault == Some(ConstructionFault::AfterPseudoConsole) {
        return Err(injected(ConstructionFault::AfterPseudoConsole));
    }

    let mut attr_size: usize = 0;
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
    }
    let mut attr_list = AttributeListGuard::new(attr_size);
    if unsafe { InitializeProcThreadAttributeList(attr_list.as_mut_ptr(), 1, 0, &mut attr_size) }
        == 0
    {
        return Err(ConstructionStepError {
            stage: "initialize_proc_thread_attribute_list",
            error: last_os("InitializeProcThreadAttributeList"),
        });
    }
    attr_list.mark_initialized();

    let hpcon = guard
        .hpcon
        .expect("construction guard owns the pseudoconsole");
    if unsafe {
        UpdateProcThreadAttribute(
            attr_list.as_mut_ptr(),
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpcon as *const std::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(ConstructionStepError {
            stage: "update_proc_thread_attribute",
            error: last_os("UpdateProcThreadAttribute"),
        });
    }

    if fault == Some(ConstructionFault::AfterAttributeSetup) {
        return Err(injected(ConstructionFault::AfterAttributeSetup));
    }

    let mut argv_full: Vec<String> = vec![program.to_string_lossy().into_owned()];
    argv_full.extend(args.iter().cloned());
    let cmd_line = build_windows_cmd_line(&argv_full);
    let mut cmd_wide = wide(&cmd_line);

    let mut si_ex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si_ex.StartupInfo.hStdInput = std::ptr::null_mut();
    si_ex.StartupInfo.hStdOutput = std::ptr::null_mut();
    si_ex.StartupInfo.hStdError = std::ptr::null_mut();
    si_ex.lpAttributeList = attr_list.as_mut_ptr();

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
    attr_list.delete_now();
    if cp_ok == 0 {
        return Err(ConstructionStepError {
            stage: "create_process",
            error: last_os("CreateProcessW"),
        });
    }

    // The guard owns the client process and its primary thread from here on.
    guard.adopt_client(pi.hProcess, pi.hThread, pi.dwProcessId);

    if fault == Some(ConstructionFault::AfterProcessCreate) {
        return Err(injected(ConstructionFault::AfterProcessCreate));
    }
    if fault == Some(ConstructionFault::BeforeResume) {
        return Err(injected(ConstructionFault::BeforeResume));
    }

    if unsafe { ResumeThread(pi.hThread) } == u32::MAX {
        return Err(ConstructionStepError {
            stage: "resume_thread",
            error: last_os("ResumeThread"),
        });
    }
    guard.release_primary_thread();

    if fault == Some(ConstructionFault::OutputWorkerSpawn) {
        return Err(injected(ConstructionFault::OutputWorkerSpawn));
    }

    // Output drain worker owns the output read handle for the session life.
    let out_handle = guard.out_read.expect("construction guard owns out_read");
    let transcript = Arc::clone(&guard.transcript);
    let close_returned = Arc::clone(&guard.close_returned);
    match spawn_output_drain(out_handle, &transcript, &close_returned) {
        Ok((thread, exit_rx)) => guard.adopt_output_worker(thread, exit_rx),
        Err(error) => {
            return Err(ConstructionStepError {
                stage: "output_worker_spawn",
                error,
            });
        }
    }

    if fault == Some(ConstructionFault::InputWorkerSpawn) {
        return Err(injected(ConstructionFault::InputWorkerSpawn));
    }

    let input_write = guard
        .input_write
        .expect("construction guard owns input_write");
    match WindowsSyncInputWorker::spawn(input_write, "session_input") {
        Ok(worker) => guard.input_worker = Some(worker),
        Err(error) => {
            return Err(ConstructionStepError {
                stage: "input_worker_spawn",
                error,
            });
        }
    }

    Ok(())
}

impl std::fmt::Debug for WindowsConPtySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsConPtySession")
            .field("child_pid", &self.child_pid)
            .field("hpcon_live", &self.hpcon.is_some())
            .field("closed", &self.closed)
            .field("close_started", &self.close_started)
            .field("close_timed_out", &self.close_timed_out)
            .field("caller_thread_id", &self.caller_thread_id)
            .field("close_thread_id", &self.close_thread_id)
            .finish_non_exhaustive()
    }
}

impl WindowsConPtySession {
    /// Spawn `program` as a ConPTY client attached to this session's HPCON.
    ///
    /// Failures after the pseudoconsole exists are cleaned up by the bounded
    /// construction guard; the caller never runs the ConPTY close call.
    pub fn spawn(
        program: impl AsRef<Path>,
        args: &[String],
        cwd: Option<&Path>,
        rows: u16,
        cols: u16,
        deadline: Duration,
    ) -> std::io::Result<Self> {
        Self::spawn_with_fault(program, args, cwd, rows, cols, deadline, None)
            .map_err(std::io::Error::from)
    }

    /// Construction with deterministic fault injection (D2-023 §19).
    ///
    /// The fault argument is the only injection seam: no environment
    /// variables, no global state, no reliance on naturally failing calls.
    pub fn spawn_with_fault(
        program: impl AsRef<Path>,
        args: &[String],
        cwd: Option<&Path>,
        rows: u16,
        cols: u16,
        deadline: Duration,
        fault: Option<ConstructionFault>,
    ) -> Result<Self, ConPtyConstructionFailure> {
        let started = Instant::now();
        let caller_thread_id = unsafe { GetCurrentThreadId() };
        if deadline.is_zero() {
            return Err(pre_hpcon_failure(
                "deadline",
                std::io::Error::other("deadline must be > 0"),
                caller_thread_id,
                started.elapsed(),
                true,
            ));
        }

        let mut in_read: HANDLE = std::ptr::null_mut();
        let mut in_write: HANDLE = std::ptr::null_mut();
        let mut out_read: HANDLE = std::ptr::null_mut();
        let mut out_write: HANDLE = std::ptr::null_mut();
        unsafe {
            if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
                return Err(pre_hpcon_failure(
                    "create_pipe_in",
                    last_os("create_pipe_in"),
                    caller_thread_id,
                    started.elapsed(),
                    true,
                ));
            }
            if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                CloseHandle(in_read);
                CloseHandle(in_write);
                return Err(pre_hpcon_failure(
                    "create_pipe_out",
                    last_os("create_pipe_out"),
                    caller_thread_id,
                    started.elapsed(),
                    true,
                ));
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
            return Err(pre_hpcon_failure(
                "create_pseudoconsole",
                std::io::Error::other(format!("CreatePseudoConsole failed: {res:#x}")),
                caller_thread_id,
                started.elapsed(),
                true,
            ));
        }

        // The pseudoconsole exists: from here ONE cleanup authority owns it.
        let mut guard = ConPtyConstructionGuard::new(hpcon, in_write, out_read, caller_thread_id);
        match build_client(&mut guard, fault, program.as_ref(), args, cwd) {
            Ok(()) => Ok(guard.into_session(deadline, rows, cols)),
            Err(step) => Err(ConPtyConstructionFailure {
                cleanup: guard.cleanup_bounded(step.stage),
                error: step.error,
            }),
        }
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
    ///
    /// Rejected as soon as close begins: ownership of the pseudoconsole has
    /// moved to the close worker and the session holds no usable token
    /// (D2-023 §16).
    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.resize_attempts += 1;
        if self.close_started || self.closed {
            return Err(std::io::Error::other(
                "pseudoconsole closing/closed: resize rejected",
            ));
        }
        let Some(hpcon) = self.hpcon else {
            return Err(std::io::Error::other(
                "pseudoconsole unavailable: resize rejected",
            ));
        };
        self.resize_api_calls += 1;
        let size = COORD {
            X: cols as i16,
            Y: rows as i16,
        };
        let res = unsafe { ResizePseudoConsole(hpcon, size) };
        if res != 0 {
            return Err(std::io::Error::other(format!(
                "ResizePseudoConsole failed: {res:#x}"
            )));
        }
        self.rows = rows;
        self.cols = cols;
        Ok(())
    }

    /// How many times callers asked to resize (including rejected calls).
    pub fn resize_attempt_count(&self) -> u32 {
        self.resize_attempts
    }

    /// How many times the Win32 resize API was actually invoked.
    ///
    /// The difference from [`Self::resize_attempt_count`] proves rejected
    /// resizes never reached the platform (D2-023 §23).
    pub fn resize_api_call_count(&self) -> u32 {
        self.resize_api_calls
    }

    /// True while this session still holds a usable pseudoconsole token.
    pub fn has_live_hpcon(&self) -> bool {
        self.hpcon.is_some()
    }

    /// Thread id that spawned this session (for close-off-caller evidence).
    pub fn caller_thread_id(&self) -> u32 {
        self.caller_thread_id
    }

    /// Thread id observed running the ConPTY close, if close has begun.
    pub fn close_thread_id(&self) -> Option<u32> {
        self.close_thread_id
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

    /// Initiate the bounded ConPTY close without waiting for it.
    ///
    /// HPCON ownership transfers to the close worker at this moment; every
    /// later operation needing the pseudoconsole is rejected (D2-023 §15).
    pub fn begin_close(&mut self) {
        self.begin_close_locked();
    }

    fn begin_close_locked(&mut self) {
        if self.close_started {
            return;
        }
        let Some(hpcon) = self.hpcon else {
            // No live pseudoconsole: nothing to transfer or close.
            return;
        };
        let close_returned = Arc::clone(&self.close_returned);
        match CloseWorkerHandle::spawn(hpcon, None, close_returned) {
            Ok(close_worker) => {
                // Ownership transfer: this session no longer holds a usable
                // pseudoconsole token from this instant onward.
                self.hpcon = None;
                self.close_started = true;
                self.close_worker = Some(close_worker);
            }
            Err(_) => {
                // The caller thread must never close the pseudoconsole
                // (D2-022/D2-023). Leave close unstarted so a later bounded
                // attempt can retry; shutdown records this as not started.
                self.close_started = false;
            }
        }
    }

    fn wait_close(&mut self, bound: Duration) -> bool {
        let Some(mut close_worker) = self.close_worker.take() else {
            // No pending worker: report the observed close fact, never a guess.
            return self.close_started && self.close_returned.load(Ordering::SeqCst);
        };
        let returned = close_worker.wait(bound);
        if let Some(thread_id) = close_worker.thread_id() {
            self.close_thread_id = Some(thread_id);
        }
        self.close_timed_out |= close_worker.timed_out;
        if returned {
            self.close_returned.store(true, Ordering::SeqCst);
            true
        } else {
            // Retain the worker for a later bounded attempt. Never join here:
            // a timeout is not permission to join (D2-023 §12).
            self.close_worker = Some(close_worker);
            false
        }
    }

    fn wait_output_worker(&mut self, bound: Duration) -> bool {
        if !self.output_running {
            return true;
        }
        let Some(exit_rx) = self.output_exit_rx.take() else {
            self.output_running = false;
            return true;
        };
        // Joins only after an observed exit signal or a channel disconnect
        // that mechanically proves the worker finished (D2-023 §12).
        let outcome = join_after_exit_signal(&exit_rx, bound, &mut self.output_thread);
        if outcome.exit_observed {
            self.output_running = false;
            true
        } else {
            // Keep the receiver for a later attempt; no join, no hang.
            self.output_exit_rx = Some(exit_rx);
            false
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
        // The output read handle is never owned here: the output drain worker
        // closes it exactly once when it exits (D2-023 §28).
        // Mark only when every owned close path completed (no leak claim on partial).
        if self.input_worker_stopped && !self.output_running {
            self.handles_closed = true;
        }
    }

    /// Explicit bounded teardown (D2-022). Returns typed evidence.
    ///
    /// Stages: stop accepting input → cancel active write → terminate/wait
    /// client → close worker (ConPTY close off this thread) → output
    /// drain continues → close completion observed → workers joined only
    /// after exit signal → single-close handles.
    pub fn shutdown_bounded(&mut self, budget: Duration) -> WindowsConPtyShutdownObservation {
        if self.closed {
            return WindowsConPtyShutdownObservation::already_closed(
                "session",
                self.caller_thread_id,
                self.close_thread_id,
            );
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
            caller_thread_id: self.caller_thread_id,
            close_thread_id: self.close_thread_id,
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
