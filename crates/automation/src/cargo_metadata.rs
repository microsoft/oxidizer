// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cargo workspace metadata discovery.

use std::path::Path;
use std::process::Command;

use ohno::{AppError, IntoAppError};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<PackageMetadata>,
}

/// Metadata for one package decoded from `cargo metadata` output.
///
/// This intentionally keeps only the fields used by repository automation.
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

/// One build target belonging to a [`PackageMetadata`] record.
#[derive(Debug, Deserialize)]
pub struct Target {
    /// Target kinds (e.g., "lib", "bin")
    pub kind: Vec<String>,
    /// Target name
    pub name: String,
}

/// Lists all workspace packages using `cargo metadata`.
///
/// # Errors
///
/// Returns an error when Cargo cannot be launched, `cargo metadata` exits
/// unsuccessfully, or its JSON output cannot be decoded.
///
/// # Examples
///
/// ```no_run
/// use automation::list_packages;
///
/// let packages = list_packages(".").expect("cargo metadata succeeds");
/// for package in packages {
///     println!("{}", package.name);
/// }
/// ```
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
