#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"
---

//! Generates the checked-in HTTP dispatch scaling fixture.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

#[path = "../tests/support/http_dispatch_scaling_codegen.rs"]
mod codegen;

const GENERATED_PATH: &str = "benches/generated/http_dispatch_scaling.rs";

enum Action {
    Generate,
    Check,
    Help,
}

fn main() -> ExitCode {
    let action = match parse_args() {
        Ok(action) => action,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    if matches!(action, Action::Help) {
        println!(
            "Usage: generate_http_dispatch_scaling.rs [--check]\n\
             \n\
             With no option, regenerate {GENERATED_PATH}.\n\
             With --check, fail when the checked-in file is stale."
        );
        return ExitCode::SUCCESS;
    }
    let generated = codegen::generated_source();
    let path = crate_root().join(GENERATED_PATH);

    if matches!(action, Action::Check) {
        return check_generated(&path, &generated);
    }

    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create {}: {error}", parent.display());
        return ExitCode::FAILURE;
    }
    if let Err(error) = fs::write(&path, generated) {
        eprintln!("failed to write {}: {error}", path.display());
        return ExitCode::FAILURE;
    }
    println!("generated {}", path.display());
    ExitCode::SUCCESS
}

fn parse_args() -> Result<Action, &'static str> {
    let mut arguments = env::args().skip(1);
    let action = match arguments.next().as_deref() {
        None => Action::Generate,
        Some("--check") => Action::Check,
        Some("--help" | "-h") => Action::Help,
        Some(_) => return Err("expected no arguments or exactly `--check`"),
    };
    if arguments.next().is_some() {
        return Err("expected no arguments or exactly `--check`");
    }
    Ok(action)
}

fn check_generated(path: &Path, generated: &str) -> ExitCode {
    match fs::read_to_string(path) {
        Ok(current) if current == generated => {
            println!("{} is current", path.display());
            ExitCode::SUCCESS
        }
        Ok(_) => {
            eprintln!(
                "{} is stale; run this generator without --check",
                path.display()
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("failed to read {}: {error}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the embedded script manifest directory is crates/routerama/scripts")
        .to_owned()
}
