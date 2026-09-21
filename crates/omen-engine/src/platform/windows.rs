use std::mem::size_of;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenThread, PROCESS_ALL_ACCESS, ResumeThread, THREAD_SUSPEND_RESUME,
};

pub struct JobObjectGuard {
    handle: HANDLE,
}

// Safety: Windows Job Object HANDLEs are thread-safe and can be sent across threads.
unsafe impl Send for JobObjectGuard {}
unsafe impl Sync for JobObjectGuard {}

impl JobObjectGuard {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err("Failed to create Job Object".into());
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let res = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };

        if res == 0 {
            unsafe { CloseHandle(handle) };
            return Err("Failed to set Job Object extended limit information".into());
        }

        Ok(Self { handle })
    }

    pub fn assign_pid(&self, pid: u32) -> Result<(), String> {
        let proc_handle = unsafe { OpenProcess(PROCESS_ALL_ACCESS, 0, pid) };
        if proc_handle.is_null() || proc_handle == INVALID_HANDLE_VALUE {
            return Err(format!("Failed to open process PID {pid}"));
        }

        let res = unsafe { AssignProcessToJobObject(self.handle, proc_handle) };
        unsafe { CloseHandle(proc_handle) };

        if res == 0 {
            return Err(format!("Failed to assign process PID {pid} to Job Object"));
        }

        Ok(())
    }

    pub fn terminate(&self, exit_code: u32) {
        unsafe {
            TerminateJobObject(self.handle, exit_code);
        }
    }

    /// Resume the suspended primary thread only after the process is in the Job Object.
    ///
    /// The process is created with CREATE_SUSPENDED, so no child code can run while
    /// this bounded thread lookup establishes the final creation barrier.
    pub fn resume_primary_thread(&self, pid: u32) -> Result<(), String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Failed to snapshot threads for suspended process PID {pid}"
            ));
        }

        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut found = None;
        let first_ok = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        if first_ok {
            loop {
                if entry.th32OwnerProcessID == pid {
                    found = Some(entry.th32ThreadID);
                    break;
                }
                if unsafe { Thread32Next(snapshot, &mut entry) } == 0 {
                    break;
                }
            }
        }
        unsafe { CloseHandle(snapshot) };

        let thread_id = found.ok_or_else(|| {
            format!("Failed to locate primary thread for suspended process PID {pid}")
        })?;
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if thread.is_null() || thread == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Failed to open primary thread {thread_id} for process PID {pid}"
            ));
        }
        let result = unsafe { ResumeThread(thread) };
        unsafe { CloseHandle(thread) };
        if result == u32::MAX {
            return Err(format!(
                "Failed to resume primary thread {thread_id} for process PID {pid}"
            ));
        }
        Ok(())
    }
}

impl Drop for JobObjectGuard {
    fn drop(&mut self) {
        if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}
