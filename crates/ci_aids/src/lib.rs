// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared code for writing Rust scripts

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    workspace_members: Vec<String>,
    packages: Vec<PackageMetadata>,
}

#[derive(Debug, Deserialize)]
struct PackageMetadata {
    name: String,
    id: String,
    #[serde(default, deserialize_with = "deserialize_publish")]
    publish: bool,
}

fn deserialize_publish<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum PublishValue {
        Flag(bool),
        Registries(Vec<String>),
    }

    match PublishValue::deserialize(deserializer) {
        Ok(PublishValue::Flag(false)) => Ok(false),
        Ok(PublishValue::Flag(true)) => Ok(true),
        Ok(PublishValue::Registries(regs)) => Ok(!regs.is_empty()),
        Err(_) => Ok(true), // Default to publishable if field is missing
    }
}

/// Represents a workspace package
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Package name
    pub name: String,
    /// Whether the package is publishable (publish != false)
    pub is_publishable: bool,
}

/// List all workspace packages using `cargo metadata`
///
/// # Errors
///
/// Returns an error if:
/// - `cargo metadata` command fails to execute
/// - The output cannot be parsed as valid JSON
pub fn list_packages(workspace_root: impl AsRef<Path>) -> Result<Vec<Package>> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--no-deps")
        .current_dir(workspace_root.as_ref())
        .output()
        .context("Failed to execute cargo metadata")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cargo metadata failed: {}", stderr);
    }

    let metadata: CargoMetadata =
        serde_json::from_slice(&output.stdout).context("Failed to parse cargo metadata output")?;

    // Extract package info from workspace members
    let workspace_member_ids: std::collections::HashSet<_> =
        metadata.workspace_members.iter().collect();

    let packages: Vec<Package> = metadata
        .packages
        .into_iter()
        .filter(|pkg| workspace_member_ids.contains(&pkg.id))
        .map(|pkg| Package {
            name: pkg.name,
            is_publishable: pkg.publish,
        })
        .collect();

    Ok(packages)
}

/// Run a cargo command and return the output
///
/// # Errors
///
/// Returns an error if:
/// - The cargo command fails to execute
/// - The command exits with a non-zero status code
///
/// # Examples
///
/// ```no_run
/// use ci_aids::run_cargo;
///
/// # fn main() -> anyhow::Result<()> {
/// // Run cargo with arguments
/// let output = run_cargo(&["check", "--all-features"])?;
/// println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
/// # Ok(())
/// # }
/// ```
pub fn run_cargo(args: impl Iterator<Item = impl AsRef<OsStr>>) -> Result<Output> {
    let args: Vec<_> = args.map(|s| s.as_ref().to_os_string()).collect();
    let args_str = args.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");

    println!("cargo {args_str}");

    let output = Command::new("cargo")
        .args(&args)
        .output()
        .context("Failed to execute cargo command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!(
            "cargo {} failed with exit code {:?}\nstdout: {}\nstderr: {}",
            args_str,
            output.status.code(),
            stdout,
            stderr
        );
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_packages() {
        // Test from the workspace root
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();

        let packages = list_packages(workspace_root).expect("Failed to list packages");

        // Verify we got some packages
        assert!(!packages.is_empty(), "Should find workspace packages");

        // Verify ci_aids itself is in the list and not publishable
        let ci_aids = packages.iter().find(|p| p.name == "ci_aids");
        assert!(ci_aids.is_some(), "ci_aids should be in workspace");
        assert!(!ci_aids.unwrap().is_publishable, "ci_aids should not be publishable");
    }
}
