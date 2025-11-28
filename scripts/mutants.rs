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
    let workspace_root = Path::new(env!("CARGO_SCRIPT_BASE_PATH")).parent().unwrap();
    let all_packages = ci_aids::list_packages(workspace_root)?;

    // Initial test groups
    let mut test_groups: Vec<Vec<String>> = vec![
        vec!["bytesbuf".to_string()],
        vec!["data_privacy".to_string(), "data_privacy_macros".to_string()],
        vec!["fundle".to_string(), "fundle_macros".to_string(), "fundle_macros_impl".to_string()],
        vec!["ohno".to_string(), "ohno_macros".to_string()],
        vec!["thread_aware".to_string(), "thread_aware_macros".to_string(), "thread_aware_macros_impl".to_string()],
    ];

    let mut publishable_packages = Vec::new();
    let mut skipped_packages = Vec::new();
    
    for pkg in all_packages {
        if pkg.is_publishable {
            publishable_packages.push(pkg.name);
        } else {
            skipped_packages.push(pkg.name);
        }
    }

    // Dynamically expand test groups with packages not in any group
    let initial_group_count = test_groups.len();
    for package_name in &publishable_packages {
        if !test_groups.iter().any(|group| group.contains(package_name)) {
            test_groups.push(vec![package_name.clone()]);
        }
    }
    let added_count = test_groups.len() - initial_group_count;

    // Log configuration
    println!("=== Mutants Testing Configuration ===");
    println!();
    println!("Settings:");
    println!("  Jobs: {}", JOBS);
    println!("  Build timeout: {}s", BUILD_TIMEOUT_SEC);
    println!("  Test timeout: {}s", TIMEOUT_SEC);
    println!("  Minimum test timeout: {}s", MINIMUM_TEST_TIMEOUT_SEC);
    println!();
    println!("Test groups:");
    for (i, group) in test_groups.iter().enumerate() {
        println!("  Group {}: [{}]", i + 1, group.join(", "));
    }
    println!();
    
    if !skipped_packages.is_empty() {
        println!("Skipped packages (not publishable):");
        for package_name in &skipped_packages {
            println!("  - {package_name}");
        }
        println!();
    }

    if added_count > 0 {
        println!("Note: {added_count} package(s) not in predefined groups were added as individual groups");
        println!();
    }
    
    println!("Starting mutants testing...");
    println!("=====================================");
    println!();

    // Test each group
    for group in &test_groups {
        let group_refs: Vec<&str> = group.iter().map(String::as_str).collect();
        mutate_group(&group_refs)?;
    }

    Ok(())
}

fn mutate_group(group: &[&str]) -> Result<()> {
    let crates = group.join(",");
    println!("Mutating group: {}", crates);

    let mut args = vec![
        "mutants",
        "--no-shuffle",
        "--baseline=skip",
        "--colors=never",
    ];

    let jobs = format!("--jobs={}", JOBS);
    let build_timeout = format!("--build-timeout={}", BUILD_TIMEOUT_SEC);
    let timeout = format!("--timeout={}", TIMEOUT_SEC);
    let min_timeout = format!("--minimum-test-timeout={}", MINIMUM_TEST_TIMEOUT_SEC);

    args.extend_from_slice(&[
        &jobs,
        &build_timeout,
        &timeout,
        &min_timeout,
        "-vV",
    ]);

    // Add package arguments
    let package_args: Vec<String> = group
        .iter()
        .map(|pkg| format!("--package={}", pkg))
        .collect();
    
    let package_refs: Vec<&str> = package_args.iter().map(String::as_str).collect();
    args.extend_from_slice(&package_refs);

    println!("Running command: cargo {}", args.join(" "));

    ci_aids::run_cargo(&args)?;

    Ok(())
}
