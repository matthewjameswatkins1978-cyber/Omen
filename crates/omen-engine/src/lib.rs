//! Omen physical process execution engine.

pub mod backend;
pub mod platform;
pub mod pty;
pub mod supervisor;

pub use backend::{
    BackendAvailability, BackendCapabilities, BackendDescriptor, BackendKind, BackendRegistry,
    ExecutionBackend, ExecutionHandle, NativeExecutionBackend, NativeExecutionHandle,
    PtyExecutionHandle, PtyExecutionRequest, WslExecutionBackend, create_platform_backend,
    to_wsl_path,
};
pub use pty::{
    DEFAULT_RING_BUFFER_CAPACITY, NativePtyHandle, RingBuffer, sanitize_terminal_escapes,
};
pub use supervisor::{
    DEFAULT_INLINE_BUDGET, ExecutionOutput, ExecutionRequest, ExecutionSecret, ProcessSupervisor,
};

/// Checks whether an OS process with the given PID is currently active.
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            if ok == 0 {
                return false;
            }
            const STILL_ACTIVE: u32 = 259;
            exit_code == STILL_ACTIVE
        }
    }

    #[cfg(unix)]
    {
        if let Some(pid_val) = rustix::process::Pid::from_raw(pid as i32) {
            rustix::process::test_kill_process(pid_val).is_ok()
        } else {
            false
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

/// Force-terminates an OS process with the given PID.
pub fn kill_process(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let ok = TerminateProcess(handle, 1);
            CloseHandle(handle);
            ok != 0
        }
    }

    #[cfg(unix)]
    {
        if let Some(pid_val) = rustix::process::Pid::from_raw(pid as i32) {
            rustix::process::kill_process(pid_val, rustix::process::Signal::KILL).is_ok()
        } else {
            false
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}
