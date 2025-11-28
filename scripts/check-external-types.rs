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

use std::env;
use std::path::Path;

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let toolchain = args.get(1)
        .context("Missing required toolchain argument (e.g., 'nightly-2025-08-06')")?;

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let all_packages = ci_aids::list_packages(workspace_root)?;

    let filtered_packages: Vec<_> = all_packages
        .into_iter()
        .filter(|pkg| !ci_aids::INTERNAL_CRATES.contains(&pkg.name.as_str()))
        .collect();

    println!("=== External Type Exposure Check ===\n");
    println!("Toolchain: {}", toolchain);
    println!("Checking {} crate(s)", filtered_packages.len());
    println!("Skipped internal crates: {}\n", ci_aids::INTERNAL_CRATES.join(", "));

    let mut checked = 0;
    let mut skipped = 0;

    for pkg in &filtered_packages {
        // Check if this is a library crate by looking at the targets
        let has_lib = pkg.targets.iter().any(|t| t.kind.contains(&"lib".to_string()));

        if has_lib {
            println!("✓ Checking external types in {}", pkg.name);
            check_external_types(&pkg.manifest_path, toolchain)?;
            checked += 1;
        } else {
            println!("⊘ Skipping {} (not a library crate)", pkg.name);
            skipped += 1;
        }
    }

    println!("\n=====================================");
    println!("Summary:");
    println!("  Checked: {}", checked);
    println!("  Skipped: {}", skipped);
    println!("  Total:   {}", checked + skipped);

    Ok(())
}

fn check_external_types(manifest_path: &str, toolchain: &str) -> Result<()> {
    let args = vec![
        format!("+{}", toolchain),
        "check-external-types".to_string(),
        "--manifest-path".to_string(),
        manifest_path.to_string(),
        "--all-features".to_string(),
    ];

    ci_aids::run_cargo(args.into_iter())
}
