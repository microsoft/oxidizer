// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A cargo subcommand to detect cyclic dependencies in workspace crates.
//!
//! # Usage
//!
//! After installation, run in any cargo workspace:
//!
//! ```bash
//! cargo ensure-no-cyclic-deps
//! ```
//!
//! Or specify a manifest path:
//!
//! ```bash
//! cargo ensure-no-cyclic-deps --manifest-path path/to/Cargo.toml
//! ```
//!
//! The tool will exit with code 0 if no cycles are found, or code 1 if cycles are detected.

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, PackageId};
use clap::Parser;
use std::collections::{HashMap, HashSet};

#[derive(Parser, Debug)]
#[command(
    name = "cargo-ensure-no-cyclic-deps",
    bin_name = "cargo",
    version,
    about = "Detects cyclic dependencies in workspace crates"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Parser, Debug)]
enum Command {
    #[command(name = "ensure-no-cyclic-deps")]
    EnsureNoCyclicDeps {
        /// Path to Cargo.toml
        #[arg(long, value_name = "PATH")]
        manifest_path: Option<std::path::PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let manifest_path = match cli.cmd {
        Some(Command::EnsureNoCyclicDeps { manifest_path }) => manifest_path,
        None => {
            // When called as `cargo-ensure-no-cyclic-deps` directly
            // (without the cargo wrapper), we still want it to work
            None
        }
    };

    let mut cmd = MetadataCommand::new();
    if let Some(path) = manifest_path {
        cmd.manifest_path(path);
    }
    // Use --no-deps to avoid Cargo resolving dependencies (which would fail on cycles)
    cmd.no_deps();

    let metadata = cmd.exec().context("Failed to load cargo metadata")?;

    let cycles = detect_cycles(&metadata);

    if cycles.is_empty() {
        println!("No cyclic dependencies found.");
        Ok(())
    } else {
        eprintln!("Error: Cyclic dependencies detected!\n");
        for (i, cycle) in cycles.iter().enumerate() {
            eprintln!("Cycle {}:", i + 1);
            eprintln!("  {}", format_cycle(cycle, &metadata));
            eprintln!();
        }
        std::process::exit(1);
    }
}

/// Detects cycles in workspace crate dependencies
fn detect_cycles(metadata: &Metadata) -> Vec<Vec<PackageId>> {
    // Build a map of workspace packages
    let workspace_package_ids: HashSet<PackageId> = metadata
        .workspace_packages()
        .iter()
        .map(|pkg| pkg.id.clone())
        .collect();

    // Build adjacency list of workspace crate dependencies
    let mut graph: HashMap<PackageId, Vec<PackageId>> = HashMap::new();

    for package in metadata.workspace_packages() {
        let mut deps = Vec::new();

        for dep in &package.dependencies {
            // Only consider workspace dependencies
            if let Some(dep_pkg) = metadata.packages.iter().find(|p| p.name == dep.name) {
                if workspace_package_ids.contains(&dep_pkg.id) {
                    deps.push(dep_pkg.id.clone());
                }
            }
        }

        graph.insert(package.id.clone(), deps);
    }

    // Find all cycles using DFS
    let mut cycles = Vec::new();
    let mut visited = HashSet::new();
    let mut rec_stack = HashSet::new();
    let mut path = Vec::new();

    for pkg_id in &workspace_package_ids {
        if !visited.contains(pkg_id) {
            dfs_find_cycles(
                pkg_id,
                &graph,
                &mut visited,
                &mut rec_stack,
                &mut path,
                &mut cycles,
            );
        }
    }

    cycles
}

/// DFS-based cycle detection
fn dfs_find_cycles(
    node: &PackageId,
    graph: &HashMap<PackageId, Vec<PackageId>>,
    visited: &mut HashSet<PackageId>,
    rec_stack: &mut HashSet<PackageId>,
    path: &mut Vec<PackageId>,
    cycles: &mut Vec<Vec<PackageId>>,
) {
    visited.insert(node.clone());
    rec_stack.insert(node.clone());
    path.push(node.clone());

    if let Some(neighbors) = graph.get(node) {
        for neighbor in neighbors {
            if !visited.contains(neighbor) {
                dfs_find_cycles(neighbor, graph, visited, rec_stack, path, cycles);
            } else if rec_stack.contains(neighbor) {
                // Found a cycle - extract it from the path
                let cycle_start = path.iter().position(|p| p == neighbor).expect("neighbor must be in path");
                let cycle: Vec<PackageId> = path[cycle_start..].to_vec();

                // Only add if we haven't seen this cycle before (considering rotations)
                if !is_duplicate_cycle(&cycle, cycles) {
                    cycles.push(cycle);
                }
            }
        }
    }

    path.pop();
    rec_stack.remove(node);
}

/// Check if a cycle is a duplicate (considering rotations and reversals)
fn is_duplicate_cycle(cycle: &[PackageId], existing_cycles: &[Vec<PackageId>]) -> bool {
    for existing in existing_cycles {
        if cycles_equal(cycle, existing) {
            return true;
        }
    }
    false
}

/// Check if two cycles are equal (considering rotations)
fn cycles_equal(a: &[PackageId], b: &[PackageId]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    // Check all rotations
    for start in 0..a.len() {
        let mut matches = true;
        for i in 0..a.len() {
            if a[(start + i) % a.len()] != b[i] {
                matches = false;
                break;
            }
        }
        if matches {
            return true;
        }
    }

    false
}

/// Format a cycle for display
fn format_cycle(cycle: &[PackageId], metadata: &Metadata) -> String {
    let names: Vec<String> = cycle
        .iter()
        .map(|id| {
            metadata
                .packages
                .iter()
                .find(|p| &p.id == id)
                .map_or_else(|| id.to_string(), |p| p.name.clone())
        })
        .collect();

    format!("{} -> {}", names.join(" -> "), names[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycles_equal() {
        // Create mock package IDs
        let id1 = PackageId {
            repr: "a".to_string(),
        };
        let id2 = PackageId {
            repr: "b".to_string(),
        };
        let id3 = PackageId {
            repr: "c".to_string(),
        };

        let cycle1 = vec![id1.clone(), id2.clone(), id3.clone()];
        let cycle2 = vec![id2.clone(), id3.clone(), id1.clone()]; // rotation of cycle1
        let cycle3 = vec![id1, id3, id2]; // different cycle

        assert!(cycles_equal(&cycle1, &cycle2));
        assert!(!cycles_equal(&cycle1, &cycle3));
    }

    #[test]
    fn test_is_duplicate_cycle() {
        let id1 = PackageId {
            repr: "a".to_string(),
        };
        let id2 = PackageId {
            repr: "b".to_string(),
        };
        let id3 = PackageId {
            repr: "c".to_string(),
        };

        let cycle = vec![id1.clone(), id2.clone(), id3.clone()];
        let existing = vec![vec![id2.clone(), id3.clone(), id1.clone()]]; // rotation

        assert!(is_duplicate_cycle(&cycle, &existing));

        let different_cycle = vec![id1, id3, id2];
        assert!(!is_duplicate_cycle(&different_cycle, &existing));
    }
}
