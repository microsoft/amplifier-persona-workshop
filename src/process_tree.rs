//! Native ACP process lifetime. Mirrors Instance's kernel job ownership;
//! remote services remain owned by their remote installation.
use anyhow::{Context, Result};
use std::{mem, ptr};
use tokio::process::Child;
use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};

/// Closing the last owned handle terminates the job's local descendants.
pub(crate) struct ProcessJob(usize);
impl ProcessJob {
    pub(crate) fn attach(child: &Child) -> Result<Self> {
        let process = child
            .raw_handle()
            .context("ACP process exited during launch")?;
        // SAFETY: valid OS process handle and initialized API structures. The
        // unnamed job handle is non-inheritable and is owned by this guard.
        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error())
                    .context("Cannot create ACP process job");
            }
            let guard = Self(job as usize);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error())
                    .context("Cannot configure ACP process cleanup");
            }
            if AssignProcessToJobObject(job, process as _) == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Cannot attach ACP process to its job");
            }
            Ok(guard)
        }
    }
}
impl Drop for ProcessJob {
    fn drop(&mut self) {
        // SAFETY: this guard owns exactly one non-inheritable job handle.
        unsafe {
            CloseHandle(self.0 as _);
        }
    }
}
