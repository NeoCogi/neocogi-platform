//! Direct child-process execution; no shell interpolation and no retained child on timeout.
use anyhow::{Context, Result, bail};
use std::{
    io::Read,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

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

/// A spawned browser is killed and reaped even if polling or output collection fails.
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Drain both pipes concurrently so large diagnostic output cannot deadlock the deadline.
pub(crate) fn timed_output(command: &mut Command, timeout: Duration) -> Result<Output> {
    let mut child = OwnedChild(command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?);
    let mut stdout = child.0.stdout.take().expect("piped stdout");
    let mut stderr = child.0.stderr.take().expect("piped stderr");
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.0.kill()?;
            child.0.wait()?;
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = out.join().map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
    let stderr = err.join().map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;
    let Some(status) = status else {
        bail!("{command:?} timed out after {timeout:?}");
    };
    Ok(Output { status, stdout, stderr })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rust-only subprocess fixture for pipe pressure, exit failures and deadlines.
    #[test]
    #[ignore = "invoked by the process regression with a private environment selector"]
    fn fixture() {
        match std::env::var("NEOCOGI_TOOL_PROCESS_FIXTURE").as_deref() {
            Ok("output") => {
                print!("{}", "x".repeat(200_000));
                eprint!("{}", "y".repeat(200_000));
            }
            Ok("fail") => std::process::exit(7),
            Ok("sleep") => thread::sleep(Duration::from_secs(30)),
            _ => (),
        }
    }

    /// Both output streams must drain and nonzero status must remain observable.
    #[test]
    fn drains_pipes_and_preserves_status() {
        for (mode, succeeds) in [("output", true), ("fail", false)] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "process::tests::fixture", "--ignored", "--nocapture"])
                .env("NEOCOGI_TOOL_PROCESS_FIXTURE", mode);
            let output = timed_output(&mut command, Duration::from_secs(10)).unwrap();
            assert_eq!(output.status.success(), succeeds);
            if succeeds {
                assert!(output.stdout.len() >= 200_000);
                assert!(output.stderr.len() >= 200_000);
            }
        }
    }

    /// A stuck browser must terminate within the owned deadline rather than report PASS.
    #[test]
    fn timeout_reaps_child() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process::tests::fixture", "--ignored", "--nocapture"])
            .env("NEOCOGI_TOOL_PROCESS_FIXTURE", "sleep");
        let start = Instant::now();
        assert!(
            timed_output(&mut command, Duration::from_millis(100))
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
