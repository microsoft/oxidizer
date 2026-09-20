// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enables workspace-only tests when their external fixtures are available.

use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(zygote_workspace_tests)");
    println!("cargo::rerun-if-changed=test-fixtures/targets/Cargo.toml");
    if Path::new("test-fixtures/targets/Cargo.toml").is_file() {
        println!("cargo::rustc-cfg=zygote_workspace_tests");
    }
}
