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
//! excludes packages using repeated `--exclude <package>` options. Each example runs
//! with `IS_TESTING=1` and a 30-second timeout.

use std::io::Write;
use std::process::{Command, ExitCode};
use std::time::Duration;

use argh::FromArgs;
use ohno::{AppError, IntoAppError, bail};
use automation::{Outcome, run_with_timeout};

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
    /// with `--exclude`.
    #[argh(option)]
    package: Option<String>,

    /// package name to exclude; repeat this option to exclude multiple
    /// packages (`--exclude foo --exclude bar`).
    #[argh(option)]
    exclude: Vec<String>,
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
    if args.package.is_some() && !args.exclude.is_empty() {
        bail!("--package and --exclude are mutually exclusive");
    }

    // Discover workspace packages and their example targets via cargo metadata.
    let packages = automation::list_packages(".")?;

    let excluded_packages: Vec<&str> = args.exclude.iter().map(String::as_str).collect();

    // Resolve scope: which packages to iterate locally + cargo args for the
    // workspace pre-build.
    let (packages_to_process, cargo_scope_args): (Vec<&automation::PackageMetadata>, Vec<String>) =
        if args.package.is_none() {
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
            let package = args.package.as_deref().unwrap();
            let pkg = packages
                .iter()
                .find(|p| p.name == package)
                .into_app_err_with(|| format!("package '{package}' not found in workspace"))?;
            (vec![pkg], vec!["--package".to_string(), package.to_string()])
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
