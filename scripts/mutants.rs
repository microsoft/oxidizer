#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
automation = { path = "../crates/automation" }
ohno = { path = "../crates/ohno", features = ["app_err"] }
argh = "0.1.12"
---

use std::path::{Path, PathBuf};

use ohno::AppError;
use argh::FromArgs;

const JOBS: u32 = 1;
const BUILD_TIMEOUT_SEC: u32 = 600;
const TIMEOUT_SEC: u32 = 300;
const MINIMUM_TEST_TIMEOUT_SEC: u32 = 60;

/// Run mutation testing on the workspace
#[derive(FromArgs)]
struct Args {
    /// run mutations in-place instead of in a temporary directory
    #[argh(switch)]
    in_place: bool,

    /// path to a diff file to limit mutations to changed code
    #[argh(option)]
    in_diff: Option<PathBuf>,
}

// Test groups define related packages that should be tested together during mutation testing.
// Grouping related packages (e.g., a crate and its proc macros) ensures mutations are properly
// validated by all relevant tests. Ungrouped packages are tested individually, which may miss
// mutations if their tests reside in dependent packages.
const TEST_GROUPS: &[&[&str]] = &[
    &["bytesbuf"],
    &["data_privacy", "data_privacy_macros", "data_privacy_macros_impl"],
    &["fundle", "fundle_macros", "fundle_macros_impl"],
    &["ohno", "ohno_macros"],
    &["thread_aware", "thread_aware_macros", "thread_aware_macros_impl"],
];

fn main() {
    let args: Args = argh::from_env();

    println!("Manifest dir: {}", env!("CARGO_MANIFEST_DIR"));
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let all_packages = automation::list_packages(workspace_root).expect("failed to list workspace packages");

    let mut test_groups: Vec<Vec<String>> = TEST_GROUPS
        .iter()
        .map(|group| group.iter().map(|s| s.to_string()).collect())
        .collect();

    let filtered_packages: Vec<_> = all_packages
        .into_iter()
        .filter(|pkg| !automation::INTERNAL_CRATES.contains(&pkg.name.as_str()))
        .collect();

    // Add ungrouped packages
    let initial_count = test_groups.len();
    for pkg in &filtered_packages {
        if !test_groups.iter().any(|g| g.contains(&pkg.name)) {
            eprintln!(
                "⚠️  '{}' package is not listed in any test group, it will be tested individually",
                pkg.name
            );
            test_groups.push(vec![pkg.name.clone()]);
        }
    }

    // Log configuration
    println!();
    println!("=== Mutants Testing Configuration ===");
    println!();
    println!("Settings:");
    println!("  Jobs: {JOBS}");
    println!("  Build timeout: {BUILD_TIMEOUT_SEC}s");
    println!("  Test timeout: {TIMEOUT_SEC}s");
    println!("  Min timeout: {MINIMUM_TEST_TIMEOUT_SEC}s");
    println!("  In-place: {}", args.in_place);
    if let Some(ref diff) = args.in_diff {
        println!("  Diff file: {}", diff.display());
    }
    println!("Test groups ({} total):", test_groups.len());
    for (i, group) in test_groups.iter().enumerate() {
        println!("  {}: [{}]", i + 1, group.join(", "));
    }
    println!();
    println!("Skipped: {}", automation::INTERNAL_CRATES.join(", "));

    if test_groups.len() > initial_count {
        println!();
        println!(
            "Added {} ungrouped package(s) as individual groups",
            test_groups.len() - initial_count
        );
    }

    println!();
    println!("Starting mutants testing...");
    println!("=====================================");
    println!();

    let mut failed_groups = Vec::new();

    for group in &test_groups {
        if let Err(e) = mutate_group(&group[..], &args) {
            eprintln!("❌ mutation testing failed for [{}]: {}", group.join(" "), e);
            failed_groups.push((group.clone(), e));
        }
    }

    println!();
    println!("=====================================");
    println!("Mutation Testing Complete");
    println!("=====================================");

    if failed_groups.is_empty() {
        println!("✅ All test groups passed!");
    } else {
        eprintln!("❌ {} test group(s) failed:", failed_groups.len());
        for (group, error) in &failed_groups {
            eprintln!("  - [{}]: {error}", group.join(", "));
        }
        eprintln!();
        std::process::exit(1);
    }
}

fn mutate_group(group: &[String], args: &Args) -> Result<(), AppError> {
    println!("Mutating: {}", group.join(", "));

    let mut cargo_args = vec![
        "mutants".to_owned(),
        "--no-shuffle".into(),
        "--baseline=skip".into(),
        "--colors=never".into(),
        format!("--build-timeout={BUILD_TIMEOUT_SEC}"),
        format!("--timeout={TIMEOUT_SEC}"),
        format!("--minimum-test-timeout={MINIMUM_TEST_TIMEOUT_SEC}"),
        "-vV".into(),
    ];

    if args.in_place {
        cargo_args.push("--in-place".into());
    } else {
        // argument '--jobs <JOBS>' cannot be used with '--in-place'
        cargo_args.push(format!("--jobs={JOBS}"));
    }

    if let Some(diff) = &args.in_diff {
        cargo_args.push("--in-diff".into());
        cargo_args.push(diff.display().to_string());
    }

    let package_args: Vec<_> = group.iter().map(|p| format!("--package={p}")).collect();
    cargo_args.extend(package_args);

    automation::run_cargo(cargo_args.into_iter())
}
