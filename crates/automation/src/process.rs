// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ohno::{AppError, IntoAppError};

/// Outcome of running a child process with a timeout
#[derive(Debug)]
pub enum Outcome {
    /// Process exited with a zero exit code
    Success,
    /// Process exited with a non-zero (or signal) exit code
    Failed(Option<i32>),
    /// Process was killed because it exceeded the timeout
    TimedOut,
}

/// Output captured from a child process run via [`run_with_timeout`]
#[derive(Debug)]
pub struct RunResult {
    /// How the process ended
    pub outcome: Outcome,
    /// Bytes written to stdout
    pub stdout: Vec<u8>,
    /// Bytes written to stderr
    pub stderr: Vec<u8>,
}

/// Spawns `cmd` with stdout/stderr captured, blocks until the child exits or the timeout elapses.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<RunResult, AppError> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().into_app_err("failed to spawn child process")?;

    // Drain stdout/stderr in background threads to avoid pipe-buffer-full
    // deadlocks on long-running examples.
    let mut stdout_pipe = child.stdout.take().into_app_err("child stdout missing")?;
    let mut stderr_pipe = child.stderr.take().into_app_err("child stderr missing")?;
    let stdout_handle = thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        stdout_pipe.read_to_end(&mut buf)?;
        Ok(buf)
    });
    let stderr_handle = thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        stderr_pipe.read_to_end(&mut buf)?;
        Ok(buf)
    });

    let started = Instant::now();
    let outcome = loop {
        if let Some(status) = child.try_wait().into_app_err("failed to wait for child process")? {
            break outcome_from_status(status);
        }

        let elapsed = started.elapsed();
        if elapsed >= timeout {
            match child.kill() {
                Ok(()) => {
                    child.wait().into_app_err("failed to reap timed-out child process")?;
                    break Outcome::TimedOut;
                }
                Err(kill_error) => {
                    if let Some(status) = child.try_wait().into_app_err("failed to wait for child process")? {
                        break outcome_from_status(status);
                    }
                    return Err(kill_error).into_app_err("failed to terminate timed-out child process");
                }
            }
        }

        thread::sleep(timeout.saturating_sub(elapsed).min(Duration::from_millis(10)));
    };

    // Reader threads finish once the child closes its pipes (always true after
    // natural exit or after the kill above).
    let stdout = stdout_handle
        .join()
        .map_err(|e| ohno::app_err!("stdout reader thread panicked: {e:?}"))?
        .into_app_err("failed to read child stdout")?;
    let stderr = stderr_handle
        .join()
        .map_err(|e| ohno::app_err!("stderr reader thread panicked: {e:?}"))?
        .into_app_err("failed to read child stderr")?;

    Ok(RunResult { outcome, stdout, stderr })
}

fn outcome_from_status(status: std::process::ExitStatus) -> Outcome {
    if status.success() {
        Outcome::Success
    } else {
        Outcome::Failed(status.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn captures_successful_process_output() {
        let result = run_with_timeout(fixture_command("success"), Duration::from_secs(5)).unwrap();

        assert!(matches!(result.outcome, Outcome::Success));
        assert!(String::from_utf8_lossy(&result.stdout).contains("fixture stdout"));
        assert!(String::from_utf8_lossy(&result.stderr).contains("fixture stderr"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn preserves_process_failure_code() {
        let result = run_with_timeout(fixture_command("failure"), Duration::from_secs(5)).unwrap();

        assert!(matches!(result.outcome, Outcome::Failed(Some(7))));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn terminates_timed_out_process() {
        let started = Instant::now();
        let result = run_with_timeout(fixture_command("timeout"), Duration::from_millis(20)).unwrap();

        assert!(matches!(result.outcome, Outcome::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn child_process_fixture() {
        match std::env::var("AUTOMATION_PROCESS_TEST").as_deref() {
            Ok("success") => {
                println!("fixture stdout");
                eprintln!("fixture stderr");
            }
            Ok("failure") => std::process::exit(7),
            Ok("timeout") => thread::sleep(Duration::from_secs(30)),
            _ => {}
        }
    }

    fn fixture_command(action: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process::tests::child_process_fixture", "--nocapture"])
            .env("AUTOMATION_PROCESS_TEST", action);
        command
    }
}
