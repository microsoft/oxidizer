#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
anyhow = { version = "1.0.100", default-features = false, features = ["std"] }
toml = { version = "0.9.8", default-features = false, features = ["std", "parse", "display", "serde"] }

---

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This script checks that all workspace dependencies in Cargo.toml have
//! default-features = false.

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let workspace_root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| ".".to_string());

    let cargo_toml_path = PathBuf::from(workspace_root).join("Cargo.toml");

    let content = std::fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("Failed to read {}", cargo_toml_path.display()))?;

    let parsed: toml::Value = toml::from_str(&content).context("Failed to parse Cargo.toml")?;

    let workspace = parsed
        .get("workspace")
        .context("No [workspace] section found")?;

    let dependencies = workspace
        .get("dependencies")
        .context("No [workspace.dependencies] section found")?;

    let deps_table = dependencies
        .as_table()
        .context("[workspace.dependencies] is not a table")?;

    let errors: Vec<String> = deps_table
        .iter()
        .filter_map(|(name, value)| validate_dependency(name, value))
        .collect();

    if !errors.is_empty() {
        eprintln!("❌ Found dependencies without default-features = false:\n");
        for error in &errors {
            eprintln!("{}", error);
        }
        eprintln!("\nAll workspace dependencies must have default-features = false.");
        eprintln!("Individual crates can enable features they need in their own Cargo.toml.");
        eprintln!("\nFound {} dependency validation error(s)", errors.len());
        std::process::exit(1);
    }

    println!("✅ All workspace dependencies have default-features = false");
    Ok(())
}

/// Validates a single dependency entry and returns an error message if invalid.
fn validate_dependency(name: &str, value: &toml::Value) -> Option<String> {
    // Error if it's just a version string
    if value.is_str() {
        return Some(format!(
            "  - '{}': uses simple version string, should be a table with default-features = false",
            name
        ));
    }

    // Must be a table
    let dep_table = value.as_table()?;

    // Check for default-features
    match dep_table.get("default-features") {
        Some(toml::Value::Boolean(false)) => None, // Valid!
        Some(toml::Value::Boolean(true)) => Some(format!(
            "  - '{}': has default-features = true (must be false)",
            name
        )),
        None => Some(format!(
            "  - '{}': missing default-features = false",
            name
        )),
        Some(_) => Some(format!(
            "  - '{}': default-features has unexpected value (must be boolean false)",
            name
        )),
    }
}
