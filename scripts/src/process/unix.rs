//! POSIX process groups; the leader's waitable identity is retained until group termination.
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, Command},
};

/// One tool-created process group; ordinary descendants inherit its containment.
pub(super) struct Scope {
    /// Positive unreaped leader PID is also the private process-group ID.
    group: libc::pid_t,
}
impl Scope {
    /// setpgid runs in std's spawn path before the executable can create descendants.
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let mut child = command.process_group(0).spawn()?;
        let group = match libc::pid_t::try_from(child.id()) {
            Ok(value) if value > 0 => value,
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::other("invalid child process-group identity"));
            }
        };
        Ok((child, Self { group }))
    }
    /// Signal only this owned group, before reaping can make its leader PID reusable.
    pub(super) fn terminate(&mut self) -> io::Result<()> {
        // SAFETY: group is positive and its leader remains owned and unreaped; the
        // negative value addresses this private group, never the tool's own group.
        if unsafe { libc::kill(-self.group, libc::SIGKILL) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // Darwin skips zombies in killpg1's process-group walk and returns EPERM
        // when none remain signalable. Confirm that case through libproc rather
        // than hiding a genuine denial for a live member. Keep the leader unreaped.
        #[cfg(target_os = "macos")]
        if error.raw_os_error() == Some(libc::EPERM) && self.only_zombies()? {
            return Ok(());
        }
        if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) }
    }

    /// Inspect Darwin's owned group after EPERM; incomplete/denied inspection fails.
    /// The unreaped leader must be present, and every remaining member must have
    /// exited. Ordinary live descendants remain errors, not successful cleanup.
    #[cfg(target_os = "macos")]
    fn only_zombies(&self) -> io::Result<bool> {
        // SAFETY: A null output requests a PID-count capacity estimate, not data.
        let capacity = unsafe { libc::proc_listpgrppids(self.group, std::ptr::null_mut(), 0) };
        if capacity <= 0 {
            return Err(io::Error::last_os_error());
        }
        let capacity = capacity as usize;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<libc::pid_t>())
            .and_then(|bytes| libc::c_int::try_from(bytes).ok())
            .ok_or_else(|| io::Error::other("process-group PID list is too large"))?;
        let mut members = Vec::<libc::pid_t>::new();
        members.try_reserve_exact(capacity).map_err(io::Error::other)?;
        members.resize(capacity, 0);
        // SAFETY: The initialized PID array owns exactly the advertised writable bytes.
        let count = unsafe { libc::proc_listpgrppids(self.group, members.as_mut_ptr().cast(), bytes) };
        if count <= 0 {
            return Err(io::Error::last_os_error());
        }
        if count as usize >= capacity {
            // A full buffer cannot establish whether additional live members exist.
            return Err(io::Error::other("process-group PID list may be truncated"));
        }
        let mut found_leader = false;
        for &pid in &members[..count as usize] {
            let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
            let bytes = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            // SAFETY: libproc synchronously writes the complete typed BSD record;
            // arg=1 includes zombies, and only a full result is read below.
            let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 1, info.as_mut_ptr().cast(), bytes) };
            if written == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) && pid != self.group {
                    continue; // An exited descendant may already have been reaped.
                }
                return Err(error);
            }
            if written != bytes {
                return Err(io::Error::other("incomplete process-group member record"));
            }
            // SAFETY: Success initialized the complete C record; all fields are plain values.
            let info = unsafe { info.assume_init() };
            if info.pbi_pgid != self.group as u32 || info.pbi_status != libc::SZOMB {
                return Ok(false);
            }
            found_leader |= pid == self.group;
        }
        Ok(found_leader)
    }
}

/// WNOWAIT observes only this child and preserves its PID until scope termination.
/// The tool owns reaping; it installs no automatic SIGCHLD reaper.
pub(super) fn finished(child: &mut Child) -> io::Result<bool> {
    // SAFETY: siginfo_t is a C output record with valid zeroed scalar/union storage;
    // waitid initializes it synchronously. Only its unrestricted signal number is read.
    let mut status = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    // SAFETY: this owned child's positive PID identifies only our unreaped child;
    // status is writable for the complete synchronous output record.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            &mut status,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == 0 {
        return Ok(status.si_signo != 0);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::Interrupted { Ok(false) } else { Err(error) }
}
