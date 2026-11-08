//! One process-tree owner keeps descendant cleanup separate from diagnostic collection.
use std::{
    io,
    process::{Child, Command, ExitStatus},
};
#[cfg(unix)]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(unix, windows)))]
compile_error!("Repository process supervision requires Unix process groups or Windows jobs");

/// The child remains owned until its containing process scope has been terminated.
/// Unix polling deliberately does not reap the leader, preventing process-group ID reuse.
pub(super) struct ProcessTree {
    /// None only after successful explicit reaping; errors retain cleanup authority.
    child: Option<Child>,
    /// Concrete OS containment established during spawn, before child code can create descendants.
    scope: platform::Scope,
}
impl ProcessTree {
    /// Spawn directly with all configured arguments/environment/stdio, inside its private scope.
    pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
        let (child, scope) = platform::Scope::spawn(command)?;
        Ok(Self { child: Some(child), scope })
    }
    /// Observe direct-child completion without giving up descendant cleanup authority.
    pub(super) fn finished(&mut self) -> io::Result<bool> {
        platform::finished(self.child.as_mut().expect("unreaped process owner"))
    }
    /// Terminate the containing scope even on ordinary exit, then retain the original child status.
    pub(super) fn finish(&mut self) -> io::Result<ExitStatus> {
        self.scope.terminate()?;
        let status = self.child.as_mut().expect("unreaped process owner").wait()?;
        self.child = None;
        Ok(status)
    }
}
impl Drop for ProcessTree {
    /// Every early return retains an owner capable of terminating descendants and reaping its child.
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = self.scope.terminate();
            // Direct kill is a fallback if scope termination itself failed.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
