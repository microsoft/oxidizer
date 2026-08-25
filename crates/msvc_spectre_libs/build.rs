// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build script for `msvc_spectre_libs`.
//!
//! Adds the Spectre-mitigated MSVC CRT import libraries to the link search path
//! for Windows MSVC targets so that dependents link against the
//! `/Qspectre`-hardened runtime. It is a no-op for every other target.

use std::env;
use std::path::{Path, PathBuf};

use cc::windows_registry;

// The build script reuses the library's `resolve` module. Its `pub` items are
// part of the library's public API but are unreachable in this build-script
// binary.
#[path = "src/resolve.rs"]
#[expect(unreachable_pub, reason = "shared source with the library; `pub` items are reachable there")]
mod resolve;

fn main() {
    // Re-run when the target-agnostic override changes, regardless of target.
    println!("cargo:rerun-if-env-changed=MSVC_SPECTRE_LIB_DIR");

    // A build script runs on the *host*, so a compile-time `cfg!(target_os)`
    // here would describe the host that compiles and runs this script -- not
    // the target Cargo is building. Gate on the selected target instead, which
    // Cargo passes through the `CARGO_CFG_TARGET_*` environment variables. This
    // makes every Windows MSVC target reach resolution regardless of host, and
    // sends every other target to the no-op path.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }

    if let Err(message) = add_spectre_link_search() {
        // Always surface the problem. `println!` is line-buffered, so the
        // warning reaches cargo before any `error`-feature exit below.
        println!("cargo:warning={message}");
        #[cfg(feature = "error")]
        std::process::exit(1);
    }
}

/// Adds the Spectre-mitigated CRT library directory to the link search path,
/// preferring an explicit build-system override over toolchain discovery.
///
/// # Errors
///
/// Returns a human-readable message when the Spectre library directory cannot
/// be located (missing override directory, unsupported target architecture,
/// missing `cl.exe`, or a toolchain without the Spectre libraries installed).
fn add_spectre_link_search() -> Result<(), String> {
    let target = env::var("TARGET").expect("cargo always sets TARGET for build scripts");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("cargo always sets CARGO_CFG_TARGET_ARCH for build scripts");

    let override_var = resolve::override_var_name(&target);
    println!("cargo:rerun-if-env-changed={override_var}");

    // 1. An explicit build-system override wins over discovery.
    if let Some(dir) = env::var_os(&override_var).or_else(|| env::var_os("MSVC_SPECTRE_LIB_DIR")) {
        let dir = PathBuf::from(dir);
        return if emit_link_search(&dir) {
            Ok(())
        } else {
            Err(format!(
                "the Spectre library directory `{}` provided via `{override_var}` (or `MSVC_SPECTRE_LIB_DIR`) does not exist",
                dir.display()
            ))
        };
    }

    let Some(arch) = resolve::spectre_arch(&target_arch) else {
        return Err(format!(
            "target architecture `{target_arch}` has no known Spectre-mitigated CRT; set `{override_var}` to override"
        ));
    };

    // 2. Prefer the toolchain root exported by the enlistment / `vcvars`.
    //    `VCToolsInstallDir` points straight at the MSVC build tools, so the
    //    Spectre libraries are `lib\spectre\<arch>` directly beneath it --
    //    no parent-directory climbing relative to `cl.exe`.
    println!("cargo:rerun-if-env-changed=VCToolsInstallDir");
    if let Some(vctools) = env::var_os("VCToolsInstallDir") {
        let dir = resolve::spectre_lib_dir(Path::new(&vctools), arch);
        if emit_link_search(&dir) {
            return Ok(());
        }
    }

    // 3. Fall back to discovering the toolchain through the Windows registry.
    //    `cc::windows_registry` can locate an MSVC installation for the selected
    //    target even from a non-Windows host that has one configured.
    let Some(tool) = windows_registry::find_tool(&target, "cl.exe") else {
        return Err(format!(
            "could not locate `cl.exe` for target `{target}`; set `{override_var}` to point at the Spectre library directory"
        ));
    };

    // `cl.exe` lives at `<root>\bin\Host<arch>\<arch>\cl.exe`, so its fourth
    // ancestor is the toolchain root (the same value as `VCToolsInstallDir`).
    let Some(root) = tool.path().ancestors().nth(4) else {
        return Err(format!(
            "could not derive the toolchain root from `{}`; set `{override_var}` to point at the Spectre library directory",
            tool.path().display()
        ));
    };

    let spectre_libs = resolve::spectre_lib_dir(root, arch);
    if emit_link_search(&spectre_libs) {
        Ok(())
    } else {
        Err(format!(
            "no Spectre-mitigated libraries were found at `{}`; modify the Visual Studio installation to add them, or set `{override_var}`",
            spectre_libs.display()
        ))
    }
}

/// Emits a `rustc-link-search` directive when `dir` is an existing directory.
/// Returns whether the directive was emitted.
fn emit_link_search(dir: &Path) -> bool {
    if dir.is_dir() {
        println!("cargo:rustc-link-search=native={}", dir.display());
        true
    } else {
        false
    }
}
