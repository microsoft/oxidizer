#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
automation = { path = "../crates/automation" }
ohno = { path = "../crates/ohno", features = ["app-err"] }
argh = "0.1"
---

//! Run all stand-alone example binaries in the workspace.
//!
//! Optionally restricts to a single workspace package (`--package`) or
//! excludes packages using cargo's native `--exclude` syntax via a single
//! `--exclude "--exclude foo --exclude bar"` argument. Each example runs
//! with `IS_TESTING=1` and a 30-second timeout.

use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use argh::FromArgs;
use ohno::{AppError, IntoAppError, app_err, bail};

const TIMEOUT: Duration = Duration::from_secs(30);

/// Examples that are expected to panic, hang, or require user interaction
/// and so must be skipped by this runner.
const EXCLUDED_EXAMPLES: &[&str] = &[
    // Interactive — requires user input from stdin.
    "employees",
];

/// Run all stand-alone example binaries in the workspace.
#[derive(FromArgs)]
struct Args {
    /// cargo profile to build/run with (e.g. `dev` or `release`).
    #[argh(option)]
    cargo_profile: String,

    /// run examples for a single workspace package only. Mutually exclusive
    /// with `--exclude`. Empty string means "all packages".
    #[argh(option, default = "String::new()")]
    package: String,

    /// raw cargo-style excludes string, e.g. `"--exclude foo --exclude bar"`.
    /// Matches the format produced by `impact -f cargo-excludes` and used by
    /// other CI workflow steps so the YAML can pass it through unchanged.
    /// Empty string means "no excludes".
    #[argh(option, default = "String::new()")]
    exclude: String,
}

#[derive(Debug)]
enum Outcome {
    Success,
    Failed(Option<i32>),
    TimedOut,
}

struct RunResult {
    outcome: Outcome,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn main() -> ExitCode {
    let args: Args = argh::from_env();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("✗ {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<(), AppError> {
    if !args.package.is_empty() && !args.exclude.is_empty() {
        bail!("--package and --exclude are mutually exclusive");
    }

    // Discover workspace packages and their example targets via cargo metadata.
    let packages = automation::list_packages(".")?;

    // Excluded package names parsed out of the cargo-excludes string.
    let excluded_packages: Vec<&str> = args
        .exclude
        .split_whitespace()
        .filter(|s| !s.is_empty() && *s != "--exclude")
        .collect();

    // Resolve scope: which packages to iterate locally + cargo args for the
    // workspace pre-build.
    let (packages_to_process, cargo_scope_args): (Vec<&automation::PackageMetadata>, Vec<String>) =
        if args.package.is_empty() {
            let to_process: Vec<_> = packages
                .iter()
                .filter(|p| !excluded_packages.contains(&p.name.as_str()))
                .collect();
            let mut scope = vec!["--workspace".to_string()];
            for ex in &excluded_packages {
                scope.push("--exclude".to_string());
                scope.push((*ex).to_string());
            }
            (to_process, scope)
        } else {
            let pkg = packages
                .iter()
                .find(|p| p.name == args.package)
                .into_app_err_with(|| format!("package '{}' not found in workspace", args.package))?;
            (vec![pkg], vec!["--package".to_string(), args.package.clone()])
        };

    println!(
        "Running examples for packages: {}",
        packages_to_process.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
    );
    if !excluded_packages.is_empty() {
        println!("Excluded packages: {}", excluded_packages.join(", "));
    }
    println!("Timeout per example: {} seconds", TIMEOUT.as_secs());
    println!("Cargo profile: {}", args.cargo_profile);
    println!();

    if packages_to_process.is_empty() {
        println!("No packages to process after applying excludes; nothing to do.");
        return Ok(());
    }

    // Pre-build all examples for the selected packages so the per-example
    // timeout below only covers execution (and a fingerprint check), not
    // compile + link cost. On Windows debug builds the link step alone for
    // the first example in a package can blow past the 30-second timeout.
    println!("Pre-building examples for selected packages...");
    let mut prebuild = Command::new("cargo");
    prebuild
        .arg("build")
        .arg("--examples")
        .arg("--profile")
        .arg(&args.cargo_profile)
        .arg("--all-features")
        .arg("--locked")
        .args(&cargo_scope_args);
    let status = prebuild.status().into_app_err("failed to spawn `cargo build --examples`")?;
    if !status.success() {
        bail!("Pre-build of examples failed with exit code {:?}", status.code());
    }
    println!();

    let mut total = 0usize;
    let mut successes = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for pkg in &packages_to_process {
        let example_targets: Vec<&str> = pkg
            .targets
            .iter()
            .filter(|t| t.kind.iter().any(|k| k == "example"))
            .map(|t| t.name.as_str())
            .collect();

        if example_targets.is_empty() {
            println!("No examples for package '{}'", pkg.name);
            continue;
        }

        for example_name in example_targets {
            if EXCLUDED_EXAMPLES.contains(&example_name) {
                println!("Skipping excluded example '{example_name}' in package '{}'", pkg.name);
                continue;
            }

            total += 1;
            println!("Running example '{example_name}' in package '{}'...", pkg.name);

            let mut cmd = Command::new("cargo");
            cmd.arg("run")
                .arg("--package")
                .arg(&pkg.name)
                .arg("--example")
                .arg(example_name)
                .arg("--profile")
                .arg(&args.cargo_profile)
                .arg("--all-features")
                .arg("--locked")
                .env("IS_TESTING", "1");

            let result = run_with_timeout(cmd, TIMEOUT)?;
            match result.outcome {
                Outcome::Success => {
                    println!("✓ Example '{example_name}' in package '{}' completed successfully", pkg.name);
                    successes += 1;
                }
                Outcome::Failed(code) => {
                    let code_str = code.map_or_else(|| "<signal>".to_string(), |c| c.to_string());
                    println!(
                        "✗ Example '{example_name}' in package '{}' failed with exit code {code_str}",
                        pkg.name
                    );
                    print_captured_output(&result.stdout, &result.stderr);
                    failures.push(format!("{}::{example_name} (exit code {code_str})", pkg.name));
                }
                Outcome::TimedOut => {
                    println!(
                        "✗ Example '{example_name}' in package '{}' timed out after {} seconds",
                        pkg.name,
                        TIMEOUT.as_secs()
                    );
                    print_captured_output(&result.stdout, &result.stderr);
                    failures.push(format!("{}::{example_name} (timeout)", pkg.name));
                }
            }
        }
    }

    println!();
    println!("Summary:");
    println!("  Total examples: {total}");
    println!("  Successful: {successes}");
    println!("  Failed: {}", failures.len());

    if !failures.is_empty() {
        println!();
        println!("Failed examples:");
        for f in &failures {
            println!("  - {f}");
        }
        bail!("{} example(s) failed", failures.len());
    }

    Ok(())
}

/// Spawns `cmd` with stdout/stderr captured, blocks until the child exits or
/// the timeout elapses, and kills the child on timeout. A dedicated wait thread
/// blocks on `child.wait()` and forwards the result via a channel so the caller
/// uses `recv_timeout` instead of polling with short sleeps. Captured output is
/// returned alongside the outcome so callers can print it on failure/timeout
/// only — keeping the CI log readable on the happy path while still surfacing
/// diagnostics when something goes wrong.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<RunResult, AppError> {
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
            bail!("wait thread exited unexpectedly without sending a result");
        }
    };

    // Reader threads finish once the child closes its pipes (always true after
    // natural exit or after the kill above).
    let stdout = stdout_handle
        .join()
        .map_err(|_| app_err!("stdout reader thread panicked"))?
        .into_app_err("failed to read child stdout")?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| app_err!("stderr reader thread panicked"))?
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

fn print_captured_output(stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        let mut out = std::io::stdout().lock();
        _ = out.write_all(b"--- stdout ---\n");
        _ = out.write_all(stdout);
        if !stdout.ends_with(b"\n") {
            _ = out.write_all(b"\n");
        }
    }
    if !stderr.is_empty() {
        let mut err = std::io::stderr().lock();
        _ = err.write_all(b"--- stderr ---\n");
        _ = err.write_all(stderr);
        if !stderr.ends_with(b"\n") {
            _ = err.write_all(b"\n");
        }
    }
}
