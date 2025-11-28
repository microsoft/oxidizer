#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
ci_aids = { path = "../crates/ci_aids" }
anyhow = "1.0"
---

use std::path::Path;

use anyhow::Result;

const JOBS: u32 = 1;
const BUILD_TIMEOUT_SEC: u32 = 600;
const TIMEOUT_SEC: u32 = 300;
const MINIMUM_TEST_TIMEOUT_SEC: u32 = 60;

// Test groups define related packages that should be tested together during mutation testing.
// Grouping related packages (e.g., a crate and its proc macros) ensures mutations are properly
// validated by all relevant tests. Ungrouped packages are tested individually, which may miss
// mutations if their tests reside in dependent packages.
const TEST_GROUPS: &[&[&str]] = &[
    &["bytesbuf"],
    &["data_privacy", "data_privacy_macros"],
    &["fundle", "fundle_macros", "fundle_macros_impl"],
    &["ohno", "ohno_macros"],
    &["thread_aware", "thread_aware_macros", "thread_aware_macros_impl"],
];

const PACKAGES_TO_SKIP: &[&str] = &["ci_aids", "testing_aids"];

fn main() -> Result<()> {
    println!("Manifest dir: {}", env!("CARGO_MANIFEST_DIR"));
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let all_packages = ci_aids::list_packages(workspace_root)?;

    let mut test_groups: Vec<Vec<String>> = TEST_GROUPS
        .iter()
        .map(|group| group.iter().map(|s| s.to_string()).collect())
        .collect();

    let filtered_packages: Vec<_> = all_packages
        .into_iter()
        .filter(|pkg| !PACKAGES_TO_SKIP.contains(&pkg.name.as_str()))
        .collect();

    // Add ungrouped packages
    let initial_count = test_groups.len();
    for pkg in &filtered_packages {
        if !test_groups.iter().any(|g| g.contains(&pkg.name)) {
            println!("this package is not listed in any test group: {}", pkg.name);
            test_groups.push(vec![pkg.name.clone()]);
        }
    }

    // Log configuration
    println!("=== Mutants Testing Configuration ===\n");
    println!("Settings:");
    println!("  Jobs: {JOBS}");
    println!("  Build timeout: {BUILD_TIMEOUT_SEC}s");
    println!("  Test timeout: {TIMEOUT_SEC}s");
    println!("  Min timeout: {MINIMUM_TEST_TIMEOUT_SEC}s");
    println!("Test groups ({} total):", test_groups.len());
    for (i, group) in test_groups.iter().enumerate() {
        println!("  {}: [{}]", i + 1, group.join(", "));
    }
    println!("\nSkipped: {}", PACKAGES_TO_SKIP.join(", "));

    if test_groups.len() > initial_count {
        println!("\nAdded {} ungrouped package(s) as individual groups", test_groups.len() - initial_count);
    }

    println!("\nStarting mutants testing...");
    println!("=====================================\n");

    for group in &test_groups {
        mutate_group(&group[..])?;
    }

    Ok(())
}

fn mutate_group(group: &[String]) -> Result<()> {
    println!("Mutating: {}", group.join(", "));

    let mut args = vec![
        "mutants".to_owned(), 
        "--no-shuffle".into(),
        "--baseline=skip".into(),
        "--colors=never".into(),
         format!("--jobs={JOBS}"),
         format!("--build-timeout={BUILD_TIMEOUT_SEC}"),
         format!("--timeout={TIMEOUT_SEC}"),
         format!("--minimum-test-timeout={MINIMUM_TEST_TIMEOUT_SEC}"),
         "-vV".into(),
    ];

    let package_args: Vec<_> = group.iter().map(|p| format!("--package={p}")).collect();
    args.extend(package_args);

    ci_aids::run_cargo(args.into_iter())
}
