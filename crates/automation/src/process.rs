// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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

    let pid = child.id();

    // Spawn a thread that blocks on child.wait() and forwards the exit status
    // via a channel. The calling thread then uses recv_timeout as the timer,
    // eliminating polling with short sleeps entirely.
    let (tx, rx) = mpsc::channel();
    let wait_handle = thread::spawn(move || {
        let _ = tx.send(child.wait());
    });

    let outcome = match rx.recv_timeout(timeout) {
        Ok(Ok(status)) => {
            if status.success() {
                Outcome::Success
            } else {
                Outcome::Failed(status.code())
            }
        }
        Ok(Err(e)) => return Err(e).into_app_err("failed to wait for child process"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_by_pid(pid);
            let _ = wait_handle.join();
            Outcome::TimedOut
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            ohno::bail!("wait thread exited unexpectedly without sending a result");
        }
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

/// Kills a process by its PID without requiring ownership of the
/// [`std::process::Child`] handle. Used to terminate a child whose `Child`
/// value has been moved into a wait thread.
fn kill_by_pid(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
    }
}
