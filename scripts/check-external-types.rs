#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
automation = { path = "../crates/automation" }
anyhow = "1.0"
argh = "0.1"
---

use std::path::Path;

use anyhow::Result;
use argh::FromArgs;

/// Check external types in all workspace library crates.
///
/// This script iterates through all workspace packages and runs
/// `cargo check-external-types` on library crates to verify that
/// public APIs don't expose types from private dependencies.
#[derive(FromArgs)]
struct Args {
    /// the Rust toolchain to use (e.g., 'nightly-2025-08-06')
    #[argh(positional)]
    toolchain: String,
}

fn main() {
    let args: Args = argh::from_env();

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let all_packages = automation::list_packages(workspace_root).expect("failed to list workspace packages");

    let filtered_packages: Vec<_> = all_packages
        .into_iter()
        .filter(|pkg| !automation::INTERNAL_CRATES.contains(&pkg.name.as_str()))
        .collect();

    println!();
    println!("=== External Type Exposure Check ===");
    println!();
    println!("Toolchain: {}", args.toolchain);
    println!("Checking {} crate(s)", filtered_packages.len());
    println!("Skipped internal crates: {}", automation::INTERNAL_CRATES.join(", "));
    println!();

    let mut checked = 0;
    let mut skipped = 0;

    for pkg in &filtered_packages {
        // Check if this is a library crate by looking at the targets
        let has_lib = pkg.targets.iter().any(|t| t.kind.contains(&"lib".to_string()));

        if has_lib {
            println!("Checking external types in {}", pkg.name);
            if let Err(e) = check_external_types(&pkg.manifest_path, &args.toolchain) {
                eprintln!("✗ Failed: {}: {e}", pkg.name);
                std::process::exit(1);
            }
            println!("✓ Passed: {}", pkg.name);
            checked += 1;
        } else {
            println!("⊘ Skipping {} (not a library crate)", pkg.name);
            skipped += 1;
        }
    }

    println!();
    println!("=====================================");
    println!("Summary:");
    println!("  Checked: {}", checked);
    println!("  Skipped: {}", skipped);
    println!("  Total:   {}", checked + skipped);
}

fn check_external_types(manifest_path: &str, toolchain: &str) -> Result<()> {
    let args = vec![
        format!("+{}", toolchain),
        "check-external-types".to_string(),
        "--manifest-path".to_string(),
        manifest_path.to_string(),
        "--all-features".to_string(),
    ];

    automation::run_cargo(args.into_iter())
}
