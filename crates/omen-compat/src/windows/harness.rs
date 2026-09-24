//! Compat-owned independent Windows ConPTY control harness.
//!
//! Uses CreatePseudoConsole / CreateProcessW directly via windows-sys.
//! Never uses omen-engine's ConPTY (D2-020). All I/O and waits are bounded.

use crate::windows::observe::{WinFileType, WinTranscript, WindowsConsoleDimensions};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{GetFileType, ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, GetConsoleMode, GetConsoleScreenBufferInfo,
    HPCON, ResizePseudoConsole, SetConsoleMode,
};
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

/// Outer bound default for a control scenario.
pub const WIN_SCENARIO_BUDGET: Duration = Duration::from_secs(20);
/// Cleanup bound after explicit terminate.
pub const WIN_CLEANUP_BOUND: Duration = Duration::from_secs(3);

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

/// Live Compat-owned ConPTY session with one client process.
pub struct WindowsConPtySession {
    hpcon: HPCON,
    process: HANDLE,
    /// Harness writes user-like input here (toward the ConPTY client).
    input_write: HANDLE,
    /// Harness reads ConPTY client output here.
    output_read: HANDLE,
    transcript: WinTranscript,
    child_pid: u32,
    started: Instant,
    deadline: Duration,
    rows: u16,
    cols: u16,
    closed: bool,
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

        // Bind output pipe to a worker so Peek/ReadFile cannot become an
        // unbounded test wait: completion is signalled via transcript + Drop
        // closes handles which releases the reader.
        Ok(Self {
            hpcon,
            process: pi.hProcess,
            input_write: in_write,
            output_read: out_read,
            transcript: WinTranscript::default(),
            child_pid: pi.dwProcessId,
            started: Instant::now(),
            deadline,
            rows,
            cols,
            closed: false,
        })
    }

    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    pub fn process_handle(&self) -> HANDLE {
        self.process
    }

    pub fn transcript(&self) -> &WinTranscript {
        &self.transcript
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_sub(self.started.elapsed())
    }

    /// Bounded non-blocking read: PeekNamedPipe then ReadFile only when data exists.
    fn pump_once(&mut self) -> std::io::Result<bool> {
        let mut available: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(
                self.output_read,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            // ERROR_BROKEN_PIPE / ERROR_NO_DATA → hangup
            let code = err.raw_os_error().unwrap_or(0);
            if code == 109 || code == 232 || code == 233 {
                self.transcript.eof_or_hangup = true;
                return Ok(false);
            }
            return Err(io_err("PeekNamedPipe", err));
        }
        if available == 0 {
            return Ok(false);
        }
        let mut buf = [0u8; 4096];
        let mut read_n: u32 = 0;
        let r = unsafe {
            ReadFile(
                self.output_read,
                buf.as_mut_ptr(),
                (available as usize).min(buf.len()) as u32,
                &mut read_n,
                std::ptr::null_mut(),
            )
        };
        if r == 0 {
            self.transcript.eof_or_hangup = true;
            return Ok(false);
        }
        if read_n > 0 {
            self.transcript.push(&buf[..read_n as usize]);
        }
        Ok(read_n > 0)
    }

    /// Poll-read until predicate, budget, EOF, or scenario deadline.
    pub fn pump_until(
        &mut self,
        predicate: impl Fn(&WinTranscript) -> bool,
        budget: Duration,
    ) -> std::io::Result<WinTranscript> {
        let outer = Instant::now() + budget.min(self.remaining());
        loop {
            if predicate(&self.transcript) {
                return Ok(self.transcript.clone());
            }
            let now = Instant::now();
            if now >= outer || self.remaining().is_zero() {
                self.transcript.bounded_out = true;
                return Ok(self.transcript.clone());
            }
            if !self.pump_once()? {
                if self.transcript.eof_or_hangup && predicate(&self.transcript) {
                    return Ok(self.transcript.clone());
                }
                let slice = (outer - now).min(Duration::from_millis(20));
                std::thread::sleep(slice.min(Duration::from_millis(WIN_POLL)));
            } else if predicate(&self.transcript) {
                return Ok(self.transcript.clone());
            }
        }
    }

    /// Write user-like input toward the ConPTY client (bounded by scenario).
    pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.remaining().is_zero() {
            return Err(std::io::Error::other("write after scenario deadline"));
        }
        let mut offset = 0;
        let started = Instant::now();
        while offset < bytes.len() {
            if started.elapsed() > self.remaining().min(Duration::from_secs(5)) {
                return Err(std::io::Error::other("write_input bounded-out"));
            }
            let mut written: u32 = 0;
            let ok = unsafe {
                WriteFile(
                    self.input_write,
                    bytes[offset..].as_ptr(),
                    (bytes.len() - offset) as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_os("WriteFile"));
            }
            if written == 0 {
                return Err(std::io::Error::other("WriteFile wrote 0 bytes"));
            }
            offset += written as usize;
        }
        Ok(())
    }

    /// Wait for exact-line or substring barrier.
    pub fn wait_for_text(
        &mut self,
        needle: &str,
        budget: Duration,
    ) -> std::io::Result<WinTranscript> {
        if self.transcript.contains(needle) {
            return Ok(self.transcript.clone());
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
        let ms = budget
            .min(self.remaining())
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
        unsafe {
            TerminateProcess(self.process, 1);
        }
        self.wait_exit(bound)
    }

    /// Close handles exactly once (idempotent).
    pub fn close_handles(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        unsafe {
            ClosePseudoConsole(self.hpcon);
            if !self.input_write.is_null() && self.input_write != INVALID_HANDLE_VALUE {
                CloseHandle(self.input_write);
            }
            if !self.output_read.is_null() && self.output_read != INVALID_HANDLE_VALUE {
                CloseHandle(self.output_read);
            }
            if !self.process.is_null() && self.process != INVALID_HANDLE_VALUE {
                CloseHandle(self.process);
            }
        }
        self.input_write = std::ptr::null_mut();
        self.output_read = std::ptr::null_mut();
        self.process = std::ptr::null_mut();
    }
}

impl Drop for WindowsConPtySession {
    fn drop(&mut self) {
        if !self.closed && !self.process.is_null() {
            unsafe {
                TerminateProcess(self.process, 1);
            }
            let _ = self.wait_exit(WIN_CLEANUP_BOUND);
        }
        self.close_handles();
    }
}

const WIN_POLL: u64 = 20;

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
