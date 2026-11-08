//! Direct command execution and owned process-tree deadlines, without shell interpolation.
use anyhow::{Context, Result, bail};
use std::{
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "process/tree.rs"]
mod tree;
use tree::ProcessTree;

/// Create a Rustup-dispatched Cargo command in this repository, optionally selecting a toolchain.
pub(crate) fn cargo(toolchain: Option<&str>) -> Command {
    let mut command = Command::new("cargo");
    if let Some(toolchain) = toolchain {
        command.arg(format!("+{toolchain}"));
    }
    command.current_dir(crate::root());
    command
}

/// Execute one foreground command, propagating failures while preserving live diagnostics.
pub(crate) fn run(command: &mut Command) -> Result<()> {
    eprintln!("+ {command:?}");
    let status = command.status().with_context(|| format!("start {command:?}"))?;
    anyhow::ensure!(status.success(), "{command:?} failed: {status}");
    Ok(())
}

/// Capture machine-readable stdout; Cargo diagnostics remain visible through stderr.
pub(crate) fn capture(command: &mut Command) -> Result<Vec<u8>> {
    let output = command.stderr(Stdio::inherit()).output().with_context(|| format!("start {command:?}"))?;
    anyhow::ensure!(output.status.success(), "{command:?} failed: {}", output.status);
    Ok(output.stdout)
}

/// Both output streams spool independently to owned regular files, never blocking on pipe EOF.
/// Process supervision and output collection share one deadline; no reader thread can outlive it.
pub(crate) fn timed_output(command: &mut Command, timeout: Duration) -> Result<Output> {
    let deadline = Instant::now().checked_add(timeout).context("process deadline exceeds clock range")?;
    if timeout.is_zero() {
        bail!("{command:?} timed out after {timeout:?}");
    }
    let mut stdout = tempfile::tempfile().context("create process stdout capture")?;
    let mut stderr = tempfile::tempfile().context("create process stderr capture")?;
    let child_stdout = stdout.try_clone()?;
    let child_stderr = stderr.try_clone()?;
    let spawned = ProcessTree::spawn(command.stdout(child_stdout).stderr(child_stderr));
    // Command retains configured File handles after spawn. Release those copies so
    // capture storage is owned only by this call and its supervised descendants.
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut process = spawned.with_context(|| format!("start {command:?}"))?;
    let expired = loop {
        if Instant::now() >= deadline {
            break true;
        }
        if process.finished()? {
            break false;
        }
        thread::sleep(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(20)));
    };
    // Clean up descendants on ordinary direct-child exit too, preserving that child's status.
    let status = process.finish().context("terminate and reap process scope")?;
    drop(process);
    if expired || Instant::now() >= deadline {
        bail!("{command:?} timed out after {timeout:?}");
    }
    let stdout = collected(&mut stdout, command, deadline, timeout)?;
    let stderr = collected(&mut stderr, command, deadline, timeout)?;
    if Instant::now() >= deadline {
        bail!("{command:?} timed out after {timeout:?}");
    }
    Ok(Output { status, stdout, stderr })
}

/// Read only the completed file extent, checking the same deadline between bounded chunks.
/// Regular-file reads do not wait for inherited handles to close, unlike read_to_end on a pipe.
fn collected(file: &mut std::fs::File, command: &Command, deadline: Instant, timeout: Duration) -> Result<Vec<u8>> {
    use std::io::{Read, Seek};
    file.rewind()?;
    let mut remaining = file.metadata()?.len();
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    while remaining != 0 {
        if Instant::now() >= deadline {
            bail!("{command:?} timed out after {timeout:?}");
        }
        let capacity = remaining.min(chunk.len() as u64) as usize;
        let count = match file.read(&mut chunk[..capacity]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        anyhow::ensure!(count != 0, "process capture was truncated during collection");
        output.try_reserve(count).context("reserve captured process output")?;
        output.extend_from_slice(&chunk[..count]);
        remaining -= count as u64;
    }
    Ok(output)
}

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
