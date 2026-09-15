// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

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

/// Fails if two workspace packages declare an example target with the same name.
///
/// Cargo writes every example in the workspace into one shared
/// `target/<profile>/examples/` directory, so two packages declaring the same
/// example name resolve to the same output file. Cargo only *warns* about that
/// ("output filename collision ... this may become a hard error in the future",
/// see rust-lang/cargo#6313) and then builds both targets concurrently into that
/// one path. On Windows the resulting `link.exe` race is fatal; on Linux and
/// `macOS` one binary silently overwrites the other, which is worse, because
/// nothing fails and the example that runs is not the one that was selected.
///
/// Callers should pass *every* workspace package rather than a filtered
/// selection. A collision is a property of the workspace, not of the selection,
/// so checking a subset hides it on exactly the pull requests that did not touch
/// either colliding crate.
///
/// Names are compared case-insensitively. Windows and the default
/// case-insensitive `macOS` volumes resolve `basic` and `Basic` to the same
/// path, so a case-only difference collides on exactly the platforms where the
/// consequence is worst. Cargo target names are ASCII, so ASCII folding is exact
/// rather than an approximation of how the filesystem folds case.
pub fn check_unique_example_names(packages: &[PackageMetadata]) -> Result<(), AppError> {
    let mut owners: BTreeMap<String, Vec<(&str, &str)>> = BTreeMap::new();
    for package in packages {
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == "example") {
                owners
                    .entry(target.name.to_ascii_lowercase())
                    .or_default()
                    .push((package.name.as_str(), target.name.as_str()));
            }
        }
    }

    let collisions: Vec<String> = owners
        .into_iter()
        .filter(|(_, declarations)| declarations.len() > 1)
        .map(|(folded, declarations)| {
            // Spell out each package's own casing only when the names actually
            // differ, so the common exact-duplicate message stays terse and a
            // case-only collision is not mistaken for a reporting bug.
            let spellings: BTreeSet<&str> = declarations.iter().map(|(_, example)| *example).collect();
            let detail = if spellings.len() == 1 {
                declarations.iter().map(|(package, _)| *package).collect::<Vec<_>>().join(", ")
            } else {
                declarations
                    .iter()
                    .map(|(package, example)| format!("{package} ('{example}')"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("  - '{folded}' is declared by: {detail}")
        })
        .collect();

    if collisions.is_empty() {
        return Ok(());
    }

    let count = collisions.len();
    let plural = if count == 1 { " is" } else { "s are" };
    let detail = collisions.join("\n");
    ohno::bail!(
        "example target names must be unique across the workspace, but {count} name{plural} used by more than one package:\n\
         {detail}\n\
         All examples share one output directory, so these overwrite each other. Rename one side to a name that says what \
         distinguishes it, or give it an explicit `[[example]] name = ...` in its Cargo.toml."
    );
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

    fn package(name: &str, examples: &[&str]) -> PackageMetadata {
        PackageMetadata {
            name: name.to_string(),
            id: format!("{name} 0.0.0"),
            manifest_path: format!("crates/{name}/Cargo.toml"),
            targets: examples
                .iter()
                .map(|example| Target {
                    kind: vec!["example".to_string()],
                    name: (*example).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn unique_example_names_are_accepted() {
        let packages = vec![package("alpha", &["basic", "advanced"]), package("beta", &["streaming"])];
        check_unique_example_names(&packages).expect("distinct example names must be accepted");
    }

    #[test]
    fn a_name_reused_by_two_packages_is_rejected() {
        let packages = vec![package("alpha", &["basic"]), package("beta", &["basic"])];

        let err = check_unique_example_names(&packages).expect_err("a reused example name must be rejected");
        let message = err.to_string();

        // The message has to name the example AND both owners: knowing only that
        // "something collided" leaves the reader running the build to find out.
        assert!(message.contains("'basic'"), "{message}");
        assert!(message.contains("alpha"), "{message}");
        assert!(message.contains("beta"), "{message}");
        assert!(message.contains("1 name is"), "{message}");
    }

    #[test]
    fn a_name_reused_by_three_packages_lists_all_of_them() {
        let packages = vec![
            package("alpha", &["basic"]),
            package("beta", &["basic"]),
            package("gamma", &["basic"]),
        ];

        let message = check_unique_example_names(&packages)
            .expect_err("a reused example name must be rejected")
            .to_string();

        assert!(message.contains("alpha, beta, gamma"), "{message}");
    }

    #[test]
    fn a_case_only_difference_is_still_a_collision() {
        // Windows and default macOS volumes are case-insensitive, so these two
        // resolve to one `examples/basic.exe`. Byte-comparing the names would
        // pass them and leave the race this guard exists to prevent.
        let packages = vec![package("alpha", &["Basic"]), package("beta", &["basic"])];

        let message = check_unique_example_names(&packages)
            .expect_err("a case-only difference must be rejected")
            .to_string();

        // Each side's own spelling has to appear, or the author cannot tell
        // which file to rename.
        assert!(message.contains("alpha ('Basic')"), "{message}");
        assert!(message.contains("beta ('basic')"), "{message}");
    }

    #[test]
    fn a_case_only_difference_within_one_package_is_a_collision() {
        // Cargo permits `examples/basic.rs` and `examples/Basic.rs` in one
        // package -- the names differ -- but the output paths do not.
        let packages = vec![package("alpha", &["basic", "Basic"])];

        let message = check_unique_example_names(&packages)
            .expect_err("a case-only difference within one package must be rejected")
            .to_string();

        assert!(message.contains("alpha ('basic')"), "{message}");
        assert!(message.contains("alpha ('Basic')"), "{message}");
    }

    #[test]
    fn an_exact_duplicate_message_does_not_repeat_the_name_per_package() {
        // The per-package spelling is only useful when the spellings differ;
        // adding it unconditionally would make the common case harder to read.
        let packages = vec![package("alpha", &["basic"]), package("beta", &["basic"])];

        let message = check_unique_example_names(&packages)
            .expect_err("a reused example name must be rejected")
            .to_string();

        assert!(message.contains("is declared by: alpha, beta"), "{message}");
    }

    #[test]
    fn one_package_may_reuse_a_name_across_target_kinds() {
        // A package that has both `src/bin/basic.rs` and `examples/basic.rs` is
        // not a collision: the two land in different output directories. Only
        // example-vs-example matters here.
        let mut packages = vec![package("alpha", &["basic"])];
        packages[0].targets.push(Target {
            kind: vec!["bin".to_string()],
            name: "basic".to_string(),
        });

        check_unique_example_names(&packages).expect("a bin and an example may share a name");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn this_workspace_has_no_colliding_example_names() {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let packages = list_packages(workspace_root).expect("failed to list packages");

        check_unique_example_names(&packages).expect("workspace example target names must stay unique");
    }
}
