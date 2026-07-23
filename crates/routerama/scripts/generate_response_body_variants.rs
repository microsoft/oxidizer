#!/usr/bin/env -S cargo +nightly -Zscript
---
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

[package]
edition = "2024"
---

//! Generates constant-route-count response-body variant fixtures.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

#[path = "../tests/support/response_body_variants_codegen.rs"]
mod codegen;

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
            "Usage: generate_response_body_variants.rs [--check]\n\
             \n\
             With no option, regenerate the 1/4/16-variant benchmark controls.\n\
             With --check, fail when any checked-in fixture is stale."
        );
        return ExitCode::SUCCESS;
    }

    let root = crate_root();
    for fixture in codegen::generated_fixtures() {
        let path = root.join(fixture.path);
        let result = if matches!(action, Action::Check) {
            check_generated(&path, &fixture.source)
        } else {
            write_generated(&path, &fixture.source)
        };
        if let Err(message) = result {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    }
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

fn check_generated(path: &Path, generated: &str) -> Result<(), String> {
    match fs::read_to_string(path) {
        Ok(current) if current == generated => {
            println!("{} is current", path.display());
            Ok(())
        }
        Ok(_) => Err(format!(
            "{} is stale; run this generator without --check",
            path.display()
        )),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

fn write_generated(path: &Path, generated: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, generated).map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    println!("generated {}", path.display());
    Ok(())
}

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the embedded script manifest directory is crates/routerama/scripts")
        .to_owned()
}
