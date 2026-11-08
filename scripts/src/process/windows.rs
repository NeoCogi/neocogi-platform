//! Windows jobs contain descendants from process creation; no post-spawn assignment race.
use std::{
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::{CommandExt, ProcThreadAttributeList},
    },
    process::{Child, Command},
};
use windows_sys::Win32::System::{
    JobObjects::{
        CreateJobObjectW, SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
    Threading::PROC_THREAD_ATTRIBUTE_JOB_LIST,
};

/// A noninheritable kernel job handle owns termination authority independently of child PID reuse.
pub(super) struct Scope {
    /// Closing the final owner also kills job members if an error bypasses explicit cleanup.
    job: OwnedHandle,
}
impl Scope {
    /// Atomically assign the child to its job using the workspace's pinned nightly std attribute API.
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        // SAFETY: null security/name select a private unnamed noninheritable job.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful CreateJobObjectW transfers one unique closeable handle.
        let scope = Self {
            job: unsafe { OwnedHandle::from_raw_handle(raw) },
        };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: valid owned job and initialized exact C record live through the synchronous call.
        if unsafe {
            SetInformationJobObject(
                scope.job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // Windows expects an array of job HANDLEs, not a pointer to a slice record.
        // The owned job and fixed array outlive both the attribute list and CreateProcess.
        // https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute
        let jobs = [scope.job.as_raw_handle()];
        let attributes = ProcThreadAttributeList::build()
            .attribute(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &jobs)
            .finish()?;
        let child = command.spawn_with_attributes(&attributes)?;
        Ok((child, scope))
    }
    /// Address the owned kernel object rather than a potentially recycled process ID.
    pub(super) fn terminate(&mut self) -> io::Result<()> {
        // SAFETY: the job handle remains owned; all processes in it belong to this invocation.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Windows can reap the direct child early because job cleanup is addressed by owned handle.
pub(super) fn finished(child: &mut Child) -> io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}
