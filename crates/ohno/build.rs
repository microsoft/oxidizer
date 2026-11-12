// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enables existing coverage exclusions without relying on inherited compiler flags.
//!
//! Nested trybuild builds retain instrumentation but remove `coverage_nightly`.
//! Probe the selected compiler so those builds still apply Ohno's exclusions.

use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=RUSTC");
    println!("cargo::rerun-if-env-changed=RUSTC_BOOTSTRAP");
    println!("cargo::rustc-check-cfg=cfg(ohno_has_coverage_attribute)");

    let rustc = env::var_os("RUSTC").ok_or("Cargo must provide RUSTC to the build script")?;
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("Cargo must provide OUT_DIR to the build script")?);
    let source = out_dir.join("coverage_attribute_probe.rs");
    fs::write(
        &source,
        "#![no_std]\n#![feature(coverage_attribute)]\n#[coverage(off)]\npub fn probe() {}\n",
    )?;

    let output = Command::new(rustc)
        // Match the package name for crate-scoped RUSTC_BOOTSTRAP permissions.
        .args([
            "--crate-name",
            "ohno",
            "--crate-type",
            "lib",
            "--emit",
            "metadata",
            "--error-format",
            "short",
        ])
        .arg("--out-dir")
        .arg(&out_dir)
        .arg(&source)
        .output()?;

    if output.status.success() {
        println!("cargo::rustc-cfg=ohno_has_coverage_attribute");
        return Ok(());
    }

    let stderr = String::from_utf8(output.stderr)?;
    let mut unsupported = false;
    for line in stderr.lines() {
        if line.contains("error[E0554]") || line.contains("error[E0635]") {
            unsupported = true;
        } else if line.contains("error[") || (line.starts_with("error:") && !line.starts_with("error: aborting due to")) {
            return Err(format!("coverage attribute probe failed ({}):\n{stderr}", output.status).into());
        }
    }
    if !unsupported {
        return Err(format!("coverage attribute probe failed ({}):\n{stderr}", output.status).into());
    }

    Ok(())
}
