// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An unpublished crate for shared code used for writing Rust scripts

#![allow(clippy::missing_errors_doc, reason = "this is an internal crate for scripts")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ohno::{AppError, IntoAppError};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<PackageMetadata>,
}

/// Metadata for a Cargo package
#[derive(Debug, Deserialize)]
pub struct PackageMetadata {
    /// Package name
    pub name: String,
    /// Package ID
    pub id: String,
    /// Path to the package's Cargo.toml
    pub manifest_path: String,
    /// Build targets in the package
    pub targets: Vec<Target>,
}

/// A Cargo build target
#[derive(Debug, Deserialize)]
pub struct Target {
    /// Target kinds (e.g., "lib", "bin")
    pub kind: Vec<String>,
    /// Target name
    pub name: String,
}

/// List all workspace packages using `cargo metadata`
pub fn list_packages(workspace_root: impl AsRef<Path>) -> Result<Vec<PackageMetadata>, AppError> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--no-deps")
        .current_dir(workspace_root.as_ref())
        .output()
        .into_app_err("failed to execute cargo metadata")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        ohno::bail!("cargo metadata failed: {stderr}");
    }

    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout).into_app_err("failed to parse cargo metadata output")?;

    Ok(metadata.packages)
}

/// Internal crates that should be skipped in CI checks
pub const INTERNAL_CRATES: &[&str] = &["automation", "testing_aids"];

/// Run a cargo command and pipe the output to stdout/stderr
pub fn run_cargo(args: impl Iterator<Item = impl AsRef<str>>) -> Result<(), AppError> {
    let args: Vec<_> = args.map(|s| s.as_ref().to_string()).collect();
    let args_str = args.join(" ");

    println!("cargo {args_str}");

    let output = duct::cmd("cargo", args).run()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        ohno::bail!(
            "cargo {} failed with exit code {:?}\nstdout: {}\nstderr: {}",
            args_str,
            output.status.code(),
            stdout,
            stderr
        );
    }

    Ok(())
}

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

/// Spawns `cmd` with stdout/stderr captured, blocks until the child exits or
/// the timeout elapses, and kills the child on timeout. A dedicated wait thread
/// blocks on `child.wait()` and forwards the result via a channel so the caller
/// uses `recv_timeout` instead of polling with short sleeps. Captured output is
/// returned alongside the outcome so callers can print it on failure/timeout
/// only — keeping the CI log readable on the happy path while still surfacing
/// diagnostics when something goes wrong.
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

    // Save the PID before moving `child` into the wait thread so we can send
    // a kill signal on timeout without needing ownership of `child`.
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
            // Timer fired: kill the child and wait for the wait thread to observe
            // it die so all resources are cleaned up before we return.
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
        .map_err(|_| ohno::app_err!("stdout reader thread panicked"))?
        .into_app_err("failed to read child stdout")?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| ohno::app_err!("stderr reader thread panicked"))?
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
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_list_packages() {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let packages = list_packages(workspace_root).expect("failed to list packages");
        assert!(!packages.is_empty());

        let automation = packages.iter().find(|p| p.name == "automation");
        assert!(automation.is_some(), "{packages:?}");
        assert!(!automation.unwrap().manifest_path.is_empty());
        assert!(!automation.unwrap().targets.is_empty());
    }
}
