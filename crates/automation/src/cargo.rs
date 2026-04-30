// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ohno::AppError;

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
