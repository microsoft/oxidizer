// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build-script integration for zygote-enabled executables.

use std::env;

/// Configures the final executable link for `zygote_rt`.
///
/// Call this from the application's build script after adding `zygote_rt` as a
/// build dependency with its `build` feature enabled. The helper emits linker
/// integration only; it never invokes a C compiler or another code generator.
///
/// On non-Linux targets this function intentionally does nothing.
///
/// # Panics
///
/// Panics when Cargo does not provide its documented
/// `CARGO_CFG_TARGET_OS` build-script variable.
pub fn configure() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo always sets CARGO_CFG_TARGET_OS for build scripts");
    if target_os != "linux" {
        return;
    }

    println!("cargo:rustc-link-arg=-Wl,--wrap=main");
    println!("cargo:rustc-link-arg=-Wl,--undefined=__wrap_main");
}
