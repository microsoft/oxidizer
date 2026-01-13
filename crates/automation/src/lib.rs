// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An unpublished crate for shared code used for writing Rust scripts

#![allow(clippy::missing_errors_doc, reason = "this is an internal crate for scripts")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::path::Path;
use std::process::Command;

use ohno::app::{IntoAppError, Result};
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
pub fn list_packages(workspace_root: impl AsRef<Path>) -> Result<Vec<PackageMetadata>> {
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
pub fn run_cargo(args: impl Iterator<Item = impl AsRef<str>>) -> Result<()> {
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
