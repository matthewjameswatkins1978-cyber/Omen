use omen_core::{CoreError, ProcessExit, PtySessionId};
use std::sync::{Arc, Mutex};

pub const DEFAULT_RING_BUFFER_CAPACITY: usize = 65536; // 64 KiB

/// In-memory bounded ring buffer for PTY output streams.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    capacity: usize,
    buffer: Vec<u8>,
    total_written: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: Vec::with_capacity(capacity.min(4096)),
            total_written: 0,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        self.total_written += data.len();
        if self.buffer.len() > self.capacity {
            let excess = self.buffer.len() - self.capacity;
            self.buffer.drain(..excess);
        }
    }

    pub fn read_all(&self) -> Vec<u8> {
        self.buffer.clone()
    }

    pub fn read_from(&self, offset: usize) -> (Vec<u8>, usize) {
        if offset >= self.total_written {
            return (Vec::new(), self.total_written);
        }
        let oldest_available_offset = self.total_written.saturating_sub(self.buffer.len());
        if offset <= oldest_available_offset {
            (self.buffer.clone(), self.total_written)
        } else {
            let relative_start = offset - oldest_available_offset;
            (self.buffer[relative_start..].to_vec(), self.total_written)
        }
    }

    pub fn total_written(&self) -> usize {
        self.total_written
    }
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_RING_BUFFER_CAPACITY)
    }
}

/// Sanitizes raw terminal byte output by stripping ANSI escape sequences,
/// control characters, and OSC strings before storing into logs, CAS, or agent context.
pub fn sanitize_terminal_escapes(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x1b {
            // ESC sequence
            i += 1;
            if i < raw.len() {
                match raw[i] {
                    b'[' => {
                        // CSI sequence: ESC [ ... [final byte 0x40..=0x7E]
                        i += 1;
                        while i < raw.len() && !(0x40..=0x7E).contains(&raw[i]) {
                            i += 1;
                        }
                        if i < raw.len() {
                            i += 1;
                        }
                    }
                    b']' => {
                        // OSC sequence: ESC ] ... BEL (0x07) or ST (ESC \)
                        i += 1;
                        while i < raw.len() {
                            if raw[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if raw[i] == 0x1b && i + 1 < raw.len() && raw[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {
                        i += 1;
                    }
                }
            }
        } else if raw[i] == b'\r' {
            i += 1;
        } else if raw[i] == b'\n' || raw[i] == b'\t' || raw[i] >= 0x20 {
            out.push(raw[i] as char);
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(windows)]
pub struct WinConPty {
    pub hpcon: windows_sys::Win32::System::Console::HPCON,
    pub process_handle: windows_sys::Win32::Foundation::HANDLE,
    pub job_guard: Option<crate::platform::windows::JobObjectGuard>,
}

#[cfg(windows)]
unsafe impl Send for WinConPty {}
#[cfg(windows)]
unsafe impl Sync for WinConPty {}

#[cfg(windows)]
impl Drop for WinConPty {
    fn drop(&mut self) {
        if self.process_handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
            && !self.process_handle.is_null()
        {
            unsafe {
                windows_sys::Win32::System::Console::ClosePseudoConsole(self.hpcon);
                windows_sys::Win32::Foundation::CloseHandle(self.process_handle);
            }
        }
    }
}

#[cfg(windows)]
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

#[cfg(unix)]
pub struct UnixPosixPty {
    pub master: std::os::fd::OwnedFd,
    pub child: tokio::process::Child,
    pub pgid: Option<u32>,
}

pub struct NativePtyHandle {
    pub session_id: PtySessionId,
    pub pid: Option<u32>,
    pub input_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    pub ring_buffer: Arc<Mutex<RingBuffer>>,
    pub rows: u16,
    pub cols: u16,
    #[cfg(windows)]
    pub win_conpty: Option<WinConPty>,
    #[cfg(unix)]
    pub unix_pty: Option<UnixPosixPty>,
    pub fallback_child: Option<tokio::process::Child>,
}

impl NativePtyHandle {
    pub fn spawn(req: &crate::backend::PtyExecutionRequest) -> Result<Self, CoreError> {
        #[cfg(windows)]
        {
            Self::spawn_windows_conpty(req)
        }
        #[cfg(unix)]
        {
            Self::spawn_unix_pty(req)
        }
        #[cfg(not(any(windows, unix)))]
        {
            Self::spawn_fallback(req)
        }
    }

    pub fn new(
        session_id: PtySessionId,
        mut child: tokio::process::Child,
        #[cfg(windows)] _job_guard: Option<crate::platform::windows::JobObjectGuard>,
        rows: u16,
        cols: u16,
    ) -> Self {
        let pid = child.id();
        let ring_buffer = Arc::new(Mutex::new(RingBuffer::default()));

        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        if let Some(mut stdin) = child.stdin.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                while let Some(bytes) = input_rx.recv().await {
                    if stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = stdin.flush().await;
                }
            });
        }

        if let Some(mut stdout) = child.stdout.take() {
            let rb = ring_buffer.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 4096];
                while let Ok(n) = stdout.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    rb.lock().unwrap().write(&buf[..n]);
                }
            });
        }

        Self {
            session_id,
            pid,
            input_tx: Some(input_tx),
            ring_buffer,
            rows,
            cols,
            #[cfg(windows)]
            win_conpty: None,
            #[cfg(unix)]
            unix_pty: None,
            fallback_child: Some(child),
        }
    }

    #[cfg(windows)]
    fn spawn_windows_conpty(req: &crate::backend::PtyExecutionRequest) -> Result<Self, CoreError> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
        use windows_sys::Win32::System::Console::{
            COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON,
        };
        use windows_sys::Win32::System::Pipes::CreatePipe;
        use windows_sys::Win32::System::Threading::{
            CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
        };

        let mut in_read: HANDLE = std::ptr::null_mut();
        let mut in_write: HANDLE = std::ptr::null_mut();
        let mut out_read: HANDLE = std::ptr::null_mut();
        let mut out_write: HANDLE = std::ptr::null_mut();

        unsafe {
            if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
                return Err(CoreError::ExecutionFailed(
                    "Failed to create PTY input pipe".into(),
                ));
            }
            if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                CloseHandle(in_read);
                CloseHandle(in_write);
                return Err(CoreError::ExecutionFailed(
                    "Failed to create PTY output pipe".into(),
                ));
            }
        }

        let size = COORD {
            X: req.cols as i16,
            Y: req.rows as i16,
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
            return Err(CoreError::ExecutionFailed(format!(
                "CreatePseudoConsole failed: {res:#x}"
            )));
        }

        let mut attr_size: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
        }
        let mut attr_buf = vec![0u8; attr_size];
        let attr_list = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        let init_ok = unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) };
        if init_ok == 0 {
            unsafe {
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(CoreError::ExecutionFailed(
                "InitializeProcThreadAttributeList failed".into(),
            ));
        }

        let update_ok = unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpcon as *const std::ffi::c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if update_ok == 0 {
            unsafe {
                DeleteProcThreadAttributeList(attr_list);
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(CoreError::ExecutionFailed(
                "UpdateProcThreadAttribute failed".into(),
            ));
        }

        let mut si_ex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        si_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si_ex.StartupInfo.hStdInput = std::ptr::null_mut();
        si_ex.StartupInfo.hStdOutput = std::ptr::null_mut();
        si_ex.StartupInfo.hStdError = std::ptr::null_mut();
        si_ex.lpAttributeList = attr_list;

        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

        let cmd_line_str = build_windows_cmd_line(&req.argv);
        let mut cmd_line_wide: Vec<u16> = cmd_line_str
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let cwd_wide: Vec<u16> = req
            .cwd
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let (env_ptr, _env_buf) = if req.env.is_empty() {
            (std::ptr::null_mut(), Vec::new())
        } else {
            let mut current_env: std::collections::BTreeMap<String, String> =
                std::env::vars().collect();
            for (k, v) in &req.env {
                current_env.insert(k.clone(), v.clone());
            }
            let mut block: Vec<u16> = Vec::new();
            for (k, v) in current_env {
                let entry = format!("{k}={v}");
                block.extend(entry.encode_utf16());
                block.push(0);
            }
            block.push(0);
            (block.as_mut_ptr() as *mut std::ffi::c_void, block)
        };

        let dw_flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;

        let job_guard = crate::platform::windows::JobObjectGuard::new().map_err(|e| {
            unsafe {
                DeleteProcThreadAttributeList(attr_list);
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            CoreError::ExecutionFailed(format!("Job object initialization error: {e}"))
        })?;

        let cp_ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                cmd_line_wide.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                dw_flags,
                env_ptr,
                cwd_wide.as_ptr(),
                &si_ex.StartupInfo,
                &mut pi,
            )
        };

        unsafe {
            DeleteProcThreadAttributeList(attr_list);
        }

        if cp_ok == 0 {
            let err = std::io::Error::last_os_error();
            unsafe {
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(CoreError::ExecutionFailed(format!(
                "CreateProcessW failed for '{}': {err}",
                req.argv[0]
            )));
        }

        if let Err(e) = job_guard.assign_pid(pi.dwProcessId) {
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(pi.hProcess, 1);
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(CoreError::ExecutionFailed(format!(
                "Failed to assign PTY PID to Job Object: {e}"
            )));
        }

        if unsafe { ResumeThread(pi.hThread) } == u32::MAX {
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(pi.hProcess, 1);
                CloseHandle(pi.hThread);
                CloseHandle(pi.hProcess);
                ClosePseudoConsole(hpcon);
                CloseHandle(in_write);
                CloseHandle(out_read);
            }
            return Err(CoreError::ExecutionFailed(format!(
                "Failed to resume contained PTY PID {} after Job Object assignment",
                pi.dwProcessId
            )));
        }

        unsafe {
            CloseHandle(pi.hThread);
        }

        let ring_buffer = Arc::new(Mutex::new(RingBuffer::default()));
        let rb_clone = ring_buffer.clone();

        let out_read_val = out_read as usize;
        std::thread::Builder::new()
            .name(format!("conpty-out-{}", req.session_id))
            .spawn(move || {
                let out_handle = out_read_val as HANDLE;
                let mut buf = [0u8; 4096];
                loop {
                    let mut bytes_read: u32 = 0;
                    let ok = unsafe {
                        ReadFile(
                            out_handle,
                            buf.as_mut_ptr(),
                            buf.len() as u32,
                            &mut bytes_read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok == 0 || bytes_read == 0 {
                        break;
                    }
                    rb_clone.lock().unwrap().write(&buf[..bytes_read as usize]);
                }
                unsafe {
                    CloseHandle(out_handle);
                }
            })
            .map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to spawn reader thread: {e}"))
            })?;

        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let in_write_val = in_write as usize;
        std::thread::Builder::new()
            .name(format!("conpty-in-{}", req.session_id))
            .spawn(move || {
                let in_handle = in_write_val as HANDLE;
                while let Some(bytes) = input_rx.blocking_recv() {
                    let mut written: u32 = 0;
                    let mut offset = 0;
                    while offset < bytes.len() {
                        let to_write = (bytes.len() - offset) as u32;
                        let ok = unsafe {
                            WriteFile(
                                in_handle,
                                bytes[offset..].as_ptr(),
                                to_write,
                                &mut written,
                                std::ptr::null_mut(),
                            )
                        };
                        if ok == 0 || written == 0 {
                            break;
                        }
                        offset += written as usize;
                    }
                }
                unsafe {
                    CloseHandle(in_handle);
                }
            })
            .map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to spawn writer thread: {e}"))
            })?;

        Ok(NativePtyHandle {
            session_id: req.session_id.clone(),
            pid: Some(pi.dwProcessId),
            input_tx: Some(input_tx),
            ring_buffer,
            rows: req.rows,
            cols: req.cols,
            win_conpty: Some(WinConPty {
                hpcon,
                process_handle: pi.hProcess,
                job_guard: Some(job_guard),
            }),
            fallback_child: None,
        })
    }

    #[cfg(unix)]
    fn spawn_unix_pty(req: &crate::backend::PtyExecutionRequest) -> Result<Self, CoreError> {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        use rustix::termios::{Winsize, tcsetwinsize};
        use std::io::{Read, Write};
        use std::os::fd::OwnedFd;

        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)
            .map_err(|e| CoreError::ExecutionFailed(format!("openpt failed: {e}")))?;
        grantpt(&master).map_err(|e| CoreError::ExecutionFailed(format!("grantpt failed: {e}")))?;
        unlockpt(&master)
            .map_err(|e| CoreError::ExecutionFailed(format!("unlockpt failed: {e}")))?;

        let pts_name = ptsname(&master, Vec::new())
            .map_err(|e| CoreError::ExecutionFailed(format!("ptsname failed: {e}")))?;

        let ws = Winsize {
            ws_row: req.rows,
            ws_col: req.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let _ = tcsetwinsize(&master, ws);

        let slave_fd = rustix::fs::open(
            &pts_name,
            rustix::fs::OFlags::RDWR,
            rustix::fs::Mode::empty(),
        )
        .map_err(|e| CoreError::ExecutionFailed(format!("open slave pty failed: {e}")))?;
        let _ = tcsetwinsize(&slave_fd, ws);

        let mut cmd = tokio::process::Command::new(&req.argv[0]);
        if req.argv.len() > 1 {
            cmd.args(&req.argv[1..]);
        }
        cmd.current_dir(&req.cwd);
        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        let slave_in: OwnedFd = slave_fd
            .try_clone()
            .map_err(|e| CoreError::ExecutionFailed(format!("clone slave fd: {e}")))?;
        let slave_out: OwnedFd = slave_fd
            .try_clone()
            .map_err(|e| CoreError::ExecutionFailed(format!("clone slave fd: {e}")))?;
        let slave_err: OwnedFd = slave_fd;

        cmd.stdin(std::process::Stdio::from(slave_in));
        cmd.stdout(std::process::Stdio::from(slave_out));
        cmd.stderr(std::process::Stdio::from(slave_err));
        cmd.process_group(0);

        let child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!("Failed to spawn child process on PTY: {e}"))
        })?;

        let pid = child.id();
        let ring_buffer = Arc::new(Mutex::new(RingBuffer::default()));
        let rb_clone = ring_buffer.clone();

        let master_read = master
            .try_clone()
            .map_err(|e| CoreError::ExecutionFailed(format!("clone master fd: {e}")))?;
        std::thread::Builder::new()
            .name(format!("posix-pty-out-{}", req.session_id))
            .spawn(move || {
                let mut file = std::fs::File::from(master_read);
                let mut buf = [0u8; 4096];
                while let Ok(n) = file.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    rb_clone.lock().unwrap().write(&buf[..n]);
                }
            })
            .map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to spawn reader thread: {e}"))
            })?;

        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let master_write = master
            .try_clone()
            .map_err(|e| CoreError::ExecutionFailed(format!("clone master fd: {e}")))?;
        std::thread::Builder::new()
            .name(format!("posix-pty-in-{}", req.session_id))
            .spawn(move || {
                let mut file = std::fs::File::from(master_write);
                while let Some(bytes) = input_rx.blocking_recv() {
                    if file.write_all(&bytes).is_err() {
                        break;
                    }
                    let _ = file.flush();
                }
            })
            .map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to spawn writer thread: {e}"))
            })?;

        Ok(NativePtyHandle {
            session_id: req.session_id.clone(),
            pid,
            input_tx: Some(input_tx),
            ring_buffer,
            rows: req.rows,
            cols: req.cols,
            #[cfg(windows)]
            win_conpty: None,
            unix_pty: Some(UnixPosixPty {
                master,
                child,
                pgid: pid,
            }),
            fallback_child: None,
        })
    }

    #[allow(dead_code)]
    fn spawn_fallback(req: &crate::backend::PtyExecutionRequest) -> Result<Self, CoreError> {
        let mut cmd = tokio::process::Command::new(&req.argv[0]);
        if req.argv.len() > 1 {
            cmd.args(&req.argv[1..]);
        }
        cmd.current_dir(&req.cwd);
        for (k, v) in &req.env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::ExecutionFailed(format!(
                "PTY process spawn failed for '{}': {e}",
                req.argv[0]
            ))
        })?;
        let pid = child.id();
        let ring_buffer = Arc::new(Mutex::new(RingBuffer::default()));

        let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        if let Some(mut stdin) = child.stdin.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                while let Some(bytes) = input_rx.recv().await {
                    if stdin.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = stdin.flush().await;
                }
            });
        }

        if let Some(mut stdout) = child.stdout.take() {
            let rb = ring_buffer.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 4096];
                while let Ok(n) = stdout.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    rb.lock().unwrap().write(&buf[..n]);
                }
            });
        }

        Ok(Self {
            session_id: req.session_id.clone(),
            pid,
            input_tx: Some(input_tx),
            ring_buffer,
            rows: req.rows,
            cols: req.cols,
            #[cfg(windows)]
            win_conpty: None,
            #[cfg(unix)]
            unix_pty: None,
            fallback_child: Some(child),
        })
    }
}

impl crate::backend::PtyExecutionHandle for NativePtyHandle {
    fn session_id(&self) -> PtySessionId {
        self.session_id.clone()
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn write_input(&mut self, data: &[u8]) -> Result<(), CoreError> {
        if let Some(tx) = &self.input_tx {
            #[cfg(windows)]
            let data = {
                let mut converted = Vec::with_capacity(data.len() + 8);
                let mut i = 0;
                while i < data.len() {
                    if data[i] == b'\n' && (i == 0 || data[i - 1] != b'\r') {
                        converted.push(b'\r');
                    }
                    converted.push(data[i]);
                    i += 1;
                }
                converted
            };
            #[cfg(not(windows))]
            let data = data.to_vec();

            tx.send(data).map_err(|e| {
                CoreError::ExecutionFailed(format!("Failed to write PTY input: {e}"))
            })?;
            Ok(())
        } else {
            Err(CoreError::ExecutionFailed("PTY stdin closed".into()))
        }
    }

    fn read_output(&mut self) -> Result<Vec<u8>, CoreError> {
        Ok(self.ring_buffer.lock().unwrap().read_all())
    }

    fn read_output_from(&mut self, offset: usize) -> Result<(Vec<u8>, usize), CoreError> {
        Ok(self.ring_buffer.lock().unwrap().read_from(offset))
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), CoreError> {
        self.rows = rows;
        self.cols = cols;
        #[cfg(windows)]
        if let Some(con) = &self.win_conpty {
            let size = windows_sys::Win32::System::Console::COORD {
                X: cols as i16,
                Y: rows as i16,
            };
            let res = unsafe {
                windows_sys::Win32::System::Console::ResizePseudoConsole(con.hpcon, size)
            };
            if res != 0 {
                return Err(CoreError::ExecutionFailed(format!(
                    "ResizePseudoConsole failed: {res:#x}"
                )));
            }
        }
        #[cfg(unix)]
        if let Some(u) = &self.unix_pty {
            let ws = rustix::termios::Winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let _ = rustix::termios::tcsetwinsize(&u.master, ws);
        }
        Ok(())
    }

    fn terminate(&mut self) -> Result<(), CoreError> {
        self.input_tx.take();
        #[cfg(windows)]
        if let Some(con) = self.win_conpty.take() {
            if let Some(jg) = &con.job_guard {
                jg.terminate(1);
            }
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(con.process_handle, 1);
                windows_sys::Win32::System::Console::ClosePseudoConsole(con.hpcon);
                windows_sys::Win32::Foundation::CloseHandle(con.process_handle);
            }
        }
        #[cfg(unix)]
        if let Some(mut u) = self.unix_pty.take() {
            if let Some(pid_val) = u
                .pgid
                .and_then(|p| rustix::process::Pid::from_raw(p as i32))
            {
                let _ = rustix::process::kill_process_group(pid_val, rustix::process::Signal::KILL);
            }
            let _ = u.child.start_kill();
        }
        if let Some(mut child) = self.fallback_child.take() {
            let _ = child.start_kill();
        }
        Ok(())
    }

    fn try_wait(&mut self) -> Result<Option<ProcessExit>, CoreError> {
        #[cfg(windows)]
        if let Some(con) = &self.win_conpty {
            let mut exit_code: u32 = 0;
            let wait_res = unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(con.process_handle, 0)
            };
            if wait_res == windows_sys::Win32::Foundation::WAIT_OBJECT_0 {
                unsafe {
                    windows_sys::Win32::System::Threading::GetExitCodeProcess(
                        con.process_handle,
                        &mut exit_code,
                    );
                }
                return Ok(Some(ProcessExit {
                    code: Some(exit_code as i32),
                    signal: None,
                }));
            } else {
                return Ok(None);
            }
        }
        #[cfg(unix)]
        if let Some(u) = &mut self.unix_pty {
            match u.child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(Some(ProcessExit {
                        code: status.code(),
                        signal: None,
                    }));
                }
                Ok(None) => return Ok(None),
                Err(e) => {
                    return Err(CoreError::ExecutionFailed(format!(
                        "Failed to poll PTY child: {e}"
                    )));
                }
            }
        }
        if let Some(child) = &mut self.fallback_child {
            match child.try_wait() {
                Ok(Some(status)) => Ok(Some(ProcessExit {
                    code: status.code(),
                    signal: None,
                })),
                Ok(None) => Ok(None),
                Err(e) => Err(CoreError::ExecutionFailed(format!(
                    "Failed to poll PTY child: {e}"
                ))),
            }
        } else {
            Ok(Some(ProcessExit {
                code: Some(0),
                signal: Some("TERMINATED".into()),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_bounded_retention() {
        let mut rb = RingBuffer::new(10);
        rb.write(b"abcdefghij"); // 10 bytes
        assert_eq!(rb.read_all(), b"abcdefghij");
        assert_eq!(rb.total_written(), 10);

        rb.write(b"12345"); // 5 more bytes -> exceeds capacity
        assert_eq!(rb.read_all(), b"fghij12345");
        assert_eq!(rb.total_written(), 15);

        let (from_10, total) = rb.read_from(10);
        assert_eq!(total, 15);
        assert_eq!(from_10, b"12345");
    }

    #[test]
    fn test_escape_sanitization() {
        let input = b"\x1b[31;1mError:\x1b[0m File not found\x1b]0;Title\x07\r\nDone";
        let clean = sanitize_terminal_escapes(input);
        assert_eq!(clean, "Error: File not found\nDone");
    }
}
