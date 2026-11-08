//! Actual subprocess fixtures test process-scope cleanup independently of diagnostic EOF.
use super::*;

/// Build this binary's ignored subprocess entrypoint with an explicit private selector.
fn fixture_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process::tests::fixture", "--ignored", "--nocapture"])
        .env("NEOCOGI_TOOL_PROCESS_FIXTURE", mode);
    command
}

/// Rust-only process tree. Descendant fixtures have a finite watchdog even if cleanup regresses.
#[test]
#[ignore = "invoked by process regressions with a private environment selector"]
#[allow(
    clippy::zombie_processes,
    reason = "The early-exit fixture deliberately leaves its live descendant to the supervising process scope."
)]
fn fixture() {
    match std::env::var("NEOCOGI_TOOL_PROCESS_FIXTURE").as_deref() {
        Ok("output") => {
            print!("{}", "x".repeat(200_000));
            eprint!("{}", "y".repeat(200_000));
        }
        Ok("fail") => std::process::exit(7),
        Ok("sleep") => thread::sleep(Duration::from_secs(3)),
        Ok("leaf") => {
            let path = std::env::var_os("NEOCOGI_PROCESS_HEARTBEAT").unwrap();
            for value in 0..100u32 {
                std::fs::write(&path, value.to_string()).unwrap();
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(mode @ ("descendant-sleep" | "descendant-exit")) => {
            let mut leaf = fixture_command("leaf").spawn().unwrap();
            let path = std::path::PathBuf::from(std::env::var_os("NEOCOGI_PROCESS_HEARTBEAT").unwrap());
            let deadline = Instant::now() + Duration::from_secs(1);
            while !path.exists() {
                assert!(Instant::now() < deadline, "leaf did not start");
                thread::sleep(Duration::from_millis(2));
            }
            println!("descendant ready");
            if mode == "descendant-sleep" {
                leaf.wait().unwrap();
            }
            // The exit case intentionally leaves the still-running descendant to its supervisor.
        }
        _ => (),
    }
}

/// Both output streams exceed pipe capacity, and original nonzero status remains observable.
#[test]
fn drains_pipes_and_preserves_status() {
    for (mode, succeeds) in [("output", true), ("fail", false)] {
        let output = timed_output(&mut fixture_command(mode), Duration::from_secs(10)).unwrap();
        assert_eq!(output.status.success(), succeeds);
        if succeeds {
            assert!(output.stdout.len() >= 200_000);
            assert!(output.stderr.len() >= 200_000);
        } else {
            assert_eq!(output.status.code(), Some(7));
        }
    }
}

/// A stuck direct child is reaped under the total deadline.
#[test]
fn timeout_reaps_child() {
    let start = Instant::now();
    assert!(
        timed_output(&mut fixture_command("sleep"), Duration::from_millis(100))
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

/// Killing or observing exit of the direct child also stops a live pipe-owning descendant.
/// The leaf writes a heartbeat so the test observes execution cleanup, not merely a returned error.
#[test]
fn inherited_output_handles_do_not_extend_deadlines_or_leave_descendants_running() {
    for mode in ["descendant-sleep", "descendant-exit"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("heartbeat");
        let mut command = fixture_command(mode);
        command.env("NEOCOGI_PROCESS_HEARTBEAT", &path);
        let timeout = if mode == "descendant-sleep" {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };
        let start = Instant::now();
        let output = timed_output(&mut command, timeout);
        assert!(start.elapsed() < Duration::from_millis(900), "process/output deadline was not bounded");
        if mode == "descendant-sleep" {
            assert!(output.unwrap_err().to_string().contains("timed out"));
        } else {
            let output = output.unwrap();
            assert!(output.status.success());
            assert!(String::from_utf8_lossy(&output.stdout).contains("descendant ready"));
        }
        let stopped = std::fs::read(&path).expect("descendant must start before parent completion");
        thread::sleep(Duration::from_millis(150));
        assert_eq!(std::fs::read(&path).unwrap(), stopped, "descendant continued running after supervisor returned");
    }
}

/// Spawn errors and an expired budget cannot leave a child or retained capture files.
#[test]
fn spawn_failure_and_zero_deadline_return_errors() {
    let mut missing = Command::new("neocogi-intentionally-nonexistent-process");
    assert!(timed_output(&mut missing, Duration::from_secs(1)).is_err());
    assert!(
        timed_output(&mut fixture_command("sleep"), Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
}

/// Collection observes the original budget even when all process execution has finished.
#[test]
fn expired_deadline_rejects_output_collection() {
    use std::io::Write;
    let mut capture = tempfile::tempfile().unwrap();
    capture.write_all(&[42; 16_384]).unwrap();
    let error = collected(&mut capture, &fixture_command("output"), Instant::now(), Duration::ZERO).unwrap_err();
    assert!(error.to_string().contains("timed out"));
}
