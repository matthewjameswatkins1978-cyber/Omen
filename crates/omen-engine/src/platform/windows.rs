use std::mem::size_of;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_ALL_ACCESS};

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
