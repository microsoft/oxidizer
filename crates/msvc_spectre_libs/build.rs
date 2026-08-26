// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build script for `msvc_spectre_libs`.
//!
//! This is the adapter around the crate's build policy: it captures the
//! environment, asks `msvc_spectre_libs_build` for a plan, and carries it out.
//! Every decision lives in that crate, where it is unit tested without a real
//! toolchain; everything here is process environment, printing, and exit
//! status.

#[cfg(feature = "error")]
use std::process::exit;

use msvc_spectre_libs_build::plan::{BuildEnvironment, Plan, plan};
use msvc_spectre_libs_build::toolchain::SystemToolchain;

fn main() {
    // A failure to capture the environment is itself a diagnostic, so it flows
    // through the same reporting and exit path as a planning failure.
    let plan = BuildEnvironment::from_env().map_or_else(|error| Plan::reporting(&error), |environment| plan(&environment, &SystemToolchain));

    for name in &plan.rerun_if_env_changed {
        println!("cargo:rerun-if-env-changed={name}");
    }

    for dir in &plan.link_search {
        println!("cargo:rustc-link-search=native={dir}");
    }

    // `println!` is line-buffered, so every warning reaches cargo before the
    // `error`-feature exit below.
    for diagnostic in &plan.diagnostics {
        println!("cargo:warning={diagnostic}");
    }

    #[cfg(feature = "error")]
    if plan.failed() {
        exit(1);
    }
}
