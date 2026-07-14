#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"

[dependencies]
argh = "0.1"
---

//! Run the full multitude correctness suite locally.
//!
//! This reproduces the correctness checks that CI performs:
//!
//! * **Miri** — four borrow-model / provenance configurations
//!   (stacked borrows, tree borrows, strict provenance, many-seeds race
//!   coverage) using `cargo miri nextest run`.
//! * **Loom** — model-checked concurrency tests (`tests/loom.rs`).
//! * **Bolero** — property-based tests (`tests/bolero_multitude.rs`).
//! * **cargo-careful** — extra UB checks via `cargo careful nextest run`.
//! * **Doctests** — `cargo test --doc` (neither Miri nor nextest run doctests,
//!   so they are otherwise unexercised by this suite).
//!
//! Prerequisites:
//!   * `rustup component add miri --toolchain <nightly>`
//!   * `cargo install cargo-nextest cargo-careful`
//!
//! Usage:
//!   `scripts/correctness.rs`              — run everything
//!   `scripts/correctness.rs --miri`       — only Miri
//!   `scripts/correctness.rs --loom`       — only Loom
//!   `scripts/correctness.rs --bolero`     — only Bolero
//!   `scripts/correctness.rs --careful`    — only cargo-careful
//!   `scripts/correctness.rs --doc`        — only doctests

use std::env;
use std::path::Path;
use std::process::{Command, ExitCode};

use argh::FromArgs;

const PACKAGE: &str = "multitude";

/// Miri configurations that mirror CI (`nightly.yml` + `main.yml` extended-analysis).
const MIRI_CONFIGS: &[MiriConfig] = &[
    MiriConfig {
        label: "stacked borrows",
        flags: "",
    },
    MiriConfig {
        label: "tree borrows",
        flags: "-Zmiri-tree-borrows",
    },
    MiriConfig {
        label: "strict provenance",
        flags: "-Zmiri-strict-provenance",
    },
    MiriConfig {
        label: "race coverage",
        flags: "-Zmiri-many-seeds=0..4",
    },
];

struct MiriConfig {
    label: &'static str,
    flags: &'static str,
}

/// Run the full multitude correctness suite.
#[derive(FromArgs)]
struct Args {
    /// run only Miri checks
    #[argh(switch)]
    miri: bool,

    /// run only Loom model checking
    #[argh(switch)]
    loom: bool,

    /// run only Bolero property tests
    #[argh(switch)]
    bolero: bool,

    /// run only cargo-careful checks
    #[argh(switch)]
    careful: bool,

    /// run only doctests
    #[argh(switch)]
    doc: bool,
}

struct Toolchains {
    nightly: String,
    latest: String,
}

fn main() -> ExitCode {
    let args: Args = argh::from_env();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .ancestors()
        .find(|p| p.join("Cargo.lock").exists())
        .expect("could not locate workspace root (no Cargo.lock found)")
        .to_path_buf();

    let toolchains = load_toolchains(&workspace_root);

    // When no specific flag is set, run everything.
    let run_all = !args.miri && !args.loom && !args.bolero && !args.careful && !args.doc;

    println!();
    println!("=== Multitude Correctness Suite ===");
    println!();
    println!("Toolchains:");
    println!("  Nightly: {}", toolchains.nightly);
    println!("  Latest:  {}", toolchains.latest);

    let mut failures: Vec<String> = Vec::new();

    if run_all || args.miri {
        for config in MIRI_CONFIGS {
            run_miri(&workspace_root, &toolchains, config, &mut failures);
        }
    }

    if run_all || args.loom {
        run_loom(&workspace_root, &toolchains, &mut failures);
    }

    if run_all || args.bolero {
        run_bolero(&workspace_root, &toolchains, &mut failures);
    }

    if run_all || args.careful {
        run_careful(&workspace_root, &toolchains, &mut failures);
    }

    if run_all || args.doc {
        run_doc(&workspace_root, &toolchains, &mut failures);
    }

    println!();
    println!("=====================================");

    if failures.is_empty() {
        println!("✅ All correctness checks passed!");
        ExitCode::SUCCESS
    } else {
        eprintln!("❌ {} check(s) failed:", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        ExitCode::FAILURE
    }
}

// --- Check runners --------------------------------------------------------

fn run_miri(root: &Path, tc: &Toolchains, config: &MiriConfig, failures: &mut Vec<String>) {
    let label = format!("Miri ({})", config.label);

    let mut cmd = cargo(root, &tc.nightly);
    cmd.args(["miri", "nextest", "run", "--all-features", "-p", PACKAGE, "--lib", "--tests"]);

    // Clear inherited MIRIFLAGS for stacked-borrows (the default model);
    // override explicitly for the other configurations.
    if config.flags.is_empty() {
        cmd.env_remove("MIRIFLAGS");
    } else {
        cmd.env("MIRIFLAGS", config.flags);
    }

    run_step(&label, cmd, failures);
}

fn run_loom(root: &Path, tc: &Toolchains, failures: &mut Vec<String>) {
    // Loom tests are gated behind `#![cfg(loom)]` and must be compiled with
    // `--cfg loom` in RUSTFLAGS.  `--release` makes the exploration substantially
    // faster.  `--test-threads=1` avoids loom's internal model checker racing
    // with itself across tests.
    let mut rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        rustflags.push(' ');
    }
    rustflags.push_str("--cfg loom");

    let mut cmd = cargo(root, &tc.latest);
    cmd.args([
        "test",
        "--release",
        "-p",
        PACKAGE,
        "--test",
        "loom",
        "--locked",
        "--",
        "--test-threads=1",
    ]);
    cmd.env("RUSTFLAGS", &rustflags);

    run_step("Loom", cmd, failures);
}

fn run_bolero(root: &Path, tc: &Toolchains, failures: &mut Vec<String>) {
    // Run bolero property tests as plain `cargo test` invocations.  This
    // exercises every `bolero::check!()` target using the built-in random
    // generator.  For coverage-guided fuzzing with libfuzzer, use `just bolero`.
    let mut cmd = cargo(root, &tc.nightly);
    cmd.args(["test", "-p", PACKAGE, "--test", "bolero_multitude", "--all-features", "--locked"]);

    run_step("Bolero", cmd, failures);
}

fn run_careful(root: &Path, tc: &Toolchains, failures: &mut Vec<String>) {
    let mut cmd = cargo(root, &tc.nightly);
    cmd.args(["careful", "nextest", "run", "--all-features", "-p", PACKAGE]);

    run_step("cargo-careful", cmd, failures);
}

fn run_doc(root: &Path, tc: &Toolchains, failures: &mut Vec<String>) {
    // Neither Miri nor cargo-nextest execute doctests, so run them explicitly
    // via plain `cargo test --doc` on the latest stable toolchain.
    let mut cmd = cargo(root, &tc.latest);
    cmd.args(["test", "--doc", "-p", PACKAGE, "--all-features", "--locked"]);

    run_step("Doctests", cmd, failures);
}

// --- Helpers --------------------------------------------------------------

fn cargo(workspace_root: &Path, toolchain: &str) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg(format!("+{toolchain}"));
    cmd.current_dir(workspace_root);
    cmd
}

fn run_step(label: &str, mut cmd: Command, failures: &mut Vec<String>) {
    let display = format_cmd(&cmd);

    println!();
    println!("--- {label} ---");
    println!("{display}");

    match cmd.status() {
        Ok(status) if status.success() => {
            println!("✅ {label}");
        }
        Ok(status) => {
            eprintln!("❌ {label} (exit code: {:?})", status.code());
            failures.push(label.to_string());
        }
        Err(e) => {
            eprintln!("❌ {label}: failed to execute: {e}");
            failures.push(label.to_string());
        }
    }
}

/// Render a `Command` as a human-readable string, including any environment
/// variable overrides set via [`Command::env`].
fn format_cmd(cmd: &Command) -> String {
    let mut parts = Vec::new();

    for (key, value) in cmd.get_envs() {
        if let Some(val) = value {
            parts.push(format!("{}={}", key.to_string_lossy(), val.to_string_lossy()));
        }
    }

    parts.push(cmd.get_program().to_string_lossy().into_owned());
    for arg in cmd.get_args() {
        parts.push(arg.to_string_lossy().into_owned());
    }

    parts.join(" ")
}

/// Parse `constants.env` at the workspace root for toolchain versions.
fn load_toolchains(workspace_root: &Path) -> Toolchains {
    let env_path = workspace_root.join("constants.env");
    let content = std::fs::read_to_string(&env_path).unwrap_or_else(|e| panic!("failed to read {}: {e}", env_path.display()));

    let mut nightly = None;
    let mut latest = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "RUST_NIGHTLY" => nightly = Some(value.trim().to_string()),
                "RUST_LATEST" => latest = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }

    Toolchains {
        nightly: nightly.expect("RUST_NIGHTLY not found in constants.env"),
        latest: latest.expect("RUST_LATEST not found in constants.env"),
    }
}
