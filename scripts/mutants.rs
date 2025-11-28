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

fn main() -> Result<()> {
    println!("Manifest dir: {}", env!("CARGO_MANIFEST_DIR"));
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let all_packages = ci_aids::list_packages(workspace_root)?;

    // Test groups define groups of packages to test together for each code mutation
    // This helps reduce overall test time by sharing build/test overhead
    // 
    // If packages are not listed here, they will be tested individually which reduces the number of running tests
    // and may cause some mutations to fail if test located in another package.
    let mut test_groups: Vec<Vec<String>> = vec![
        vec!["bytesbuf".to_string()],
        vec!["data_privacy".to_string(), "data_privacy_macros".to_string()],
        vec!["fundle".to_string(), "fundle_macros".to_string(), "fundle_macros_impl".to_string()],
        vec!["ohno".to_string(), "ohno_macros".to_string()],
        vec!["thread_aware".to_string(), "thread_aware_macros".to_string(), "thread_aware_macros_impl".to_string()],
    ];

    let (publishable, skipped): (Vec<_>, Vec<_>) = all_packages
        .into_iter()
        .partition(|pkg| pkg.is_publishable);

    // Add ungrouped packages
    let initial_count = test_groups.len();
    for pkg in &publishable {
        if !test_groups.iter().any(|g| g.contains(&pkg.name)) {
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

    if !skipped.is_empty() {
        println!("\nSkipped (not publishable): {}", skipped.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "));
    }
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

    let output = ci_aids::run_cargo(args.into_iter())?;
    println!("{}", String::from_utf8_lossy(&output.stdout));
    println!("{}", String::from_utf8_lossy(&output.stderr));
    Ok(())
}
