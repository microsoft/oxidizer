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

use std::io::Read;
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

/// Spawns `cmd` with stdout/stderr captured, polls for completion, and kills
/// the child if it exceeds `timeout`. Captured output is returned alongside
/// the outcome so callers can print it on failure/timeout only — keeping the
/// CI log readable on the happy path while still surfacing diagnostics when
/// something goes wrong.
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

    let start = Instant::now();
    let outcome = loop {
        match child.try_wait().into_app_err("failed to poll child")? {
            Some(status) => {
                break if status.success() {
                    Outcome::Success
                } else {
                    Outcome::Failed(status.code())
                };
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Outcome::TimedOut;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    };

    // Reader threads finish once the child closes its pipes (always, since
    // we either let it exit naturally or killed it above).
    let stdout = stdout_handle
        .join()
        .into_app_err("stdout reader thread panicked")?
        .into_app_err("failed to read child stdout")?;
    let stderr = stderr_handle
        .join()
        .into_app_err("stderr reader thread panicked")?
        .into_app_err("failed to read child stderr")?;

    Ok(RunResult { outcome, stdout, stderr })
}

fn print_captured_output(stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        let mut stdout_lock = stdout().lock();
        _ = stdout_lock.write_all(b"--- stdout ---\n");
        _ = stdout_lock.write_all(stdout);
        if !stdout.ends_with(b"\n") {
            _ = stdout_lock.write_all(b"\n");
        }
    }
    if !stderr.is_empty() {
        let mut stderr_lock = stderr().lock();
        _ = stderr_lock.write_all(b"--- stderr ---\n");
        _ = stderr_lock.write_all(stderr);
        if !stderr.ends_with(b"\n") {
            _ = stderr_lock.write_all(b"\n");
        }
    }
}
