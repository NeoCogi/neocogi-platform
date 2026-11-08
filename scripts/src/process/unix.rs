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
        if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) }
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
