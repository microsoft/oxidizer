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

// The build script reuses the library's `resolve` and `flags` modules. Their
// `pub` items are part of the library's public API but are unreachable in this
// build-script binary.
#[path = "src/resolve.rs"]
#[expect(unreachable_pub, reason = "shared source with the library; `pub` items are reachable there")]
mod resolve;

#[path = "src/flags.rs"]
#[expect(unreachable_pub, reason = "shared source with the library; `pub` items are reachable there")]
mod flags;

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

    let failure = add_spectre_link_search().err();

    // Report the hardening flags this crate cannot deliver itself. Runs even
    // when the search-path step failed, so one build surfaces every problem.
    if let Err(message) = verify_required_link_args() {
        // Always surface the problem. `println!` is line-buffered, so the
        // warning reaches cargo before any `error`-feature exit below.
        println!("cargo:warning={message}");
        #[cfg(feature = "error")]
        std::process::exit(1);
    }

    if let Some(message) = failure {
        println!("cargo:warning={message}");
        #[cfg(feature = "error")]
        std::process::exit(1);
    }
}

/// Verifies that the linker arguments this crate cannot propagate, listed in
/// [`flags::REQUIRED_LINK_ARGS_VAR`], actually reached `rustc`.
///
/// A build script's `cargo:rustc-link-arg` applies only to the emitting
/// package's own artifacts, so these flags must come from `.cargo/config.toml`
/// or `RUSTFLAGS`. Since a `RUSTFLAGS` environment variable *replaces* the
/// `target.<triple>.rustflags` config table rather than merging with it, an
/// ambient `RUSTFLAGS` silently drops them; this check makes that visible.
///
/// Does nothing when the variable is unset or empty, which is the default.
///
/// # Errors
///
/// Returns a human-readable message listing the arguments that are missing.
fn verify_required_link_args() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed={}", flags::REQUIRED_LINK_ARGS_VAR);
    let Ok(configured) = env::var(flags::REQUIRED_LINK_ARGS_VAR) else {
        return Ok(());
    };

    let required = flags::required_link_args(&configured);
    if required.is_empty() {
        return Ok(());
    }

    // Absent (as opposed to empty) means this cargo does not report the final
    // flags, so there is nothing to verify and no basis for a warning.
    let Ok(encoded) = encoded_rustflags() else {
        return Ok(());
    };

    let missing = flags::missing_required_link_args(&encoded, &required);
    if missing.is_empty() {
        return Ok(());
    }

    Err(format!(
        "linker argument(s) `{}` required by `{}` did not reach rustc. Add them to `[target.<triple>] rustflags` in `.cargo/config.toml`. \
         If a `RUSTFLAGS` environment variable is set, note that it REPLACES the config `rustflags` rather than merging with it, so the \
         configured flags are dropped; append them to `RUSTFLAGS` as well or unset it.",
        missing.join("`, `"),
        flags::REQUIRED_LINK_ARGS_VAR
    ))
}

/// Reads the final `rustc` flags Cargo reports to build scripts, declaring the
/// re-run dependency on every call so that no caller has to rely on another
/// having already declared it.
fn encoded_rustflags() -> Result<String, env::VarError> {
    println!("cargo:rerun-if-env-changed={}", flags::CARGO_ENCODED_RUSTFLAGS_VAR);
    env::var(flags::CARGO_ENCODED_RUSTFLAGS_VAR)
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

/// Emits a `rustc-link-search` directive when `dir` is an existing directory
/// whose contents are not already on the link search path.
///
/// Returns whether `dir` is in effect afterwards -- either because the
/// directive was emitted, or because the directory was already supplied by the
/// surrounding build system. Skipping the duplicate keeps the crate resilient
/// when an integrator supplies the same directory through `RUSTFLAGS` or `LIB`:
/// a second copy would be harmless to the linker, but silently tolerating it
/// would hide the redundant configuration.
fn emit_link_search(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }

    if let Some(source) = already_on_search_path(dir) {
        // Not a `cargo:warning=`: a correctly configured build system that
        // deliberately supplies the directory itself should not produce a
        // warning on every build. Visible via `cargo build -vv`.
        println!(
            "msvc_spectre_libs: `{}` is already on the link search path via {source}; not adding it again",
            dir.display()
        );
        return true;
    }

    println!("cargo:rustc-link-search=native={}", dir.display());
    true
}

/// Returns the name of the mechanism that already places `dir` on the link
/// search path, or [`None`] when it is not there yet.
fn already_on_search_path(dir: &Path) -> Option<&'static str> {
    // `link.exe` reads `LIB` directly, so a directory listed there is already
    // searched without any `rustc` flag.
    println!("cargo:rerun-if-env-changed=LIB");
    if env::var("LIB").is_ok_and(|lib| flags::lib_var_contains(&lib, dir)) {
        return Some("the `LIB` environment variable");
    }

    // `rustc` search paths come from the flags Cargo reports back to us.
    if encoded_rustflags().is_ok_and(|encoded| flags::adds_link_search(&encoded, dir)) {
        return Some("`RUSTFLAGS`");
    }

    None
}
