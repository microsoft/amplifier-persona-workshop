//! Native ACP process lifetime. Mirrors Instance's kernel job ownership;
//! remote services remain owned by their remote installation.
use anyhow::{Context, Result, ensure};
use std::{mem, ptr};
use tokio::process::Child;
use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};

/// Closing the last owned handle terminates the job's local descendants.
pub(crate) struct ProcessJob(usize);
impl ProcessJob {
    /// The initial thread is suspended until its whole descendant tree is owned.
    pub(crate) async fn attach_suspended(child: &mut Child) -> Result<Self> {
        let result = Self::attach(child).and_then(|job| {
            resume_suspended(child)?;
            Ok(job)
        });
        match result {
            Ok(job) => Ok(job),
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                Err(error)
            }
        }
    }
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

// std::process retains the process handle but not the initial thread handle.
// CREATE_SUSPENDED guarantees that no child code runs before job assignment.
#[cfg(windows)]
fn resume_suspended(child: &Child) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    ensure!(
        snapshot != INVALID_HANDLE_VALUE,
        "Could not inspect the suspended Persona process"
    );
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut found = None;
    let mut available = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while available {
        if entry.th32OwnerProcessID == child.id().context("Child process exited before resume")? {
            found = Some(entry.th32ThreadID);
            break;
        }
        available = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe {
        CloseHandle(snapshot);
    }
    let id = found.context("The suspended Persona process has no initial thread")?;
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, id) };
    ensure!(
        !thread.is_null(),
        "Could not open the suspended Persona thread"
    );
    let resumed = unsafe { ResumeThread(thread) };
    unsafe {
        CloseHandle(thread);
    }
    ensure!(
        resumed == 1,
        "Could not resume the suspended Persona process"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn suspended_child_cannot_run_before_job_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("ran.txt");
        let python = std::env::var("PERSONA_TEST_PYTHON").unwrap_or_else(|_| "python".into());
        let mut command = tokio::process::Command::new(python);
        command.args(["-c","import pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text('ran'); time.sleep(60)"]).arg(&marker).creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED).kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!marker.exists());
        let job = ProcessJob::attach_suspended(&mut child).await.unwrap();
        for _ in 0..100 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(marker.exists());
        drop(job);
        tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
    }
}
