// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build script for `msvc_spectre_libs`.
//!
//! Adds the Spectre-mitigated MSVC CRT import libraries to the link search path
//! for Windows MSVC targets so that dependents link against the
//! `/Qspectre`-hardened runtime. It is a no-op for every other target.

use std::env::{self, VarError};
use std::path::{Path, PathBuf};
#[cfg(feature = "error")]
use std::process::exit;

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

use resolve::SpectreArch;

/// The target-agnostic Spectre library directory override, applying to every
/// target that does not have a target-specific override set.
const GENERIC_OVERRIDE_VAR: &str = "MSVC_SPECTRE_LIB_DIR";

fn main() {
    // Re-run when the target-agnostic override changes, regardless of target.
    println!("cargo:rerun-if-env-changed={GENERIC_OVERRIDE_VAR}");

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

    // Evaluate both steps before reporting anything, so a single build
    // surfaces every problem instead of one per iteration. The search-path
    // step runs first because it is the one that emits a cargo directive.
    let search_failure = add_spectre_link_search().err();
    let flags_failure = verify_required_link_args().err();

    // `println!` is line-buffered, so every warning reaches cargo before the
    // `error`-feature exit below.
    for message in [search_failure.as_ref(), flags_failure.as_ref()].into_iter().flatten() {
        println!("cargo:warning={message}");
    }

    #[cfg(feature = "error")]
    if search_failure.is_some() || flags_failure.is_some() {
        exit(1);
    }
}

/// Verifies that the linker arguments this crate cannot propagate, listed in
/// [`flags::REQUIRED_LINK_ARGS_VAR`], actually reached `rustc`.
///
/// A build script's `cargo:rustc-link-arg` applies only to the emitting
/// package's own artifacts, so these arguments must come from
/// `.cargo/config.toml` or `RUSTFLAGS`. Since a `RUSTFLAGS` environment
/// variable *replaces* the `target.<triple>.rustflags` config table rather than
/// merging with it, an ambient `RUSTFLAGS` silently drops them; this check
/// makes that visible.
///
/// Does nothing when no requirement is configured, which is the default.
///
/// # Errors
///
/// Returns a human-readable message listing the arguments that are missing, or
/// naming a configured variable whose value could not be read.
fn verify_required_link_args() -> Result<(), String> {
    let target = env::var("TARGET").expect("cargo always sets TARGET for build scripts");

    let Some((source_var, configured)) = required_link_args_value(&target)? else {
        return Ok(());
    };

    let required = flags::required_link_args(&configured);
    if required.is_empty() {
        return Ok(());
    }

    println!("cargo:rerun-if-env-changed={}", flags::CARGO_ENCODED_RUSTFLAGS_VAR);
    let encoded = match env::var(flags::CARGO_ENCODED_RUSTFLAGS_VAR) {
        Ok(encoded) => encoded,
        // Absent means this cargo does not report the final flags, so there is
        // nothing to verify and no basis for a diagnostic.
        Err(VarError::NotPresent) => return Ok(()),
        Err(VarError::NotUnicode(_)) => return Err(unreadable_var_message(flags::CARGO_ENCODED_RUSTFLAGS_VAR, "checked")),
    };

    let missing = flags::missing_required_link_args(&encoded, &required);
    if missing.is_empty() {
        return Ok(());
    }

    // Spell out the `-Clink-arg=` form: these are linker arguments, so the bare
    // token is not something rustc accepts on its own.
    let as_rustflags = missing.iter().map(|arg| format!("-Clink-arg={arg}")).collect::<Vec<_>>().join(" ");

    Err(format!(
        "linker argument(s) `{}` required by `{source_var}` did not reach rustc. Add them to `[target.<triple>] rustflags` in \
         `.cargo/config.toml` as `{as_rustflags}`. If a `RUSTFLAGS` environment variable is set, note that it REPLACES the config \
         `rustflags` rather than merging with it, so the configured flags are dropped; add `{as_rustflags}` to `RUSTFLAGS` as well, or \
         unset it.",
        missing.join("`, `")
    ))
}

/// Reads the configured required-linker-argument list for `target`.
///
/// The target-specific variable wins over the target-agnostic one, so a
/// requirement that applies to a single architecture does not produce a false
/// diagnostic on the others. Returns the name of the variable that supplied the
/// value alongside it, so a diagnostic can name the one the integrator actually
/// set. Returns [`None`] when neither is set, which disables the check.
///
/// # Errors
///
/// Returns a human-readable message when a variable is set to a value that is
/// not valid Unicode. Treating that as "unset" would silently skip a check the
/// integrator explicitly opted into.
fn required_link_args_value(target: &str) -> Result<Option<(String, String)>, String> {
    let target_var = flags::required_link_args_var_name(target);

    for name in [target_var.as_str(), flags::REQUIRED_LINK_ARGS_VAR] {
        println!("cargo:rerun-if-env-changed={name}");
        match env::var(name) {
            Ok(value) => return Ok(Some((name.to_owned(), value))),
            Err(VarError::NotPresent) => {}
            Err(VarError::NotUnicode(_)) => return Err(unreadable_var_message(name, "checked")),
        }
    }

    Ok(None)
}

/// Builds the diagnostic for a configured variable that cannot be read.
///
/// `purpose` completes the sentence "so it could not be ...".
fn unreadable_var_message(name: &str, purpose: &str) -> String {
    format!("the environment variable `{name}` is set but its value is not valid Unicode, so it could not be {purpose}")
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

    // 1. An explicit build-system override wins over discovery. Remember which
    //    variable supplied the value so a diagnostic names the one that is
    //    actually set rather than the one that merely takes precedence.
    if let Some((source_var, dir)) = configured_override_dir(&override_var)? {
        return if emit_link_search(&dir) {
            Ok(())
        } else {
            Err(format!(
                "the Spectre library directory `{}` provided via `{source_var}` does not exist",
                dir.display()
            ))
        };
    }

    let Some(arch) = SpectreArch::from_target_arch(&target_arch) else {
        return Err(format!(
            "target architecture `{target_arch}` has no known Spectre-mitigated CRT; set `{override_var}` to override"
        ));
    };

    // 2. Prefer the toolchain root exported by the build environment.
    //    `VCToolsInstallDir` points straight at the MSVC build tools, so the
    //    Spectre libraries sit directly beneath it -- no parent-directory
    //    climbing relative to `cl.exe`.
    println!("cargo:rerun-if-env-changed=VCToolsInstallDir");
    // Read as `String`: a directory that is not valid Unicode cannot be carried
    // losslessly in a cargo directive, so treat it as no answer and let
    // registry discovery run. Unlike the override variables this is not
    // something the integrator pointed at this crate, so it is not an error.
    if let Ok(vctools) = env::var("VCToolsInstallDir") {
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

    // `cl.exe` lives under a host/target compiler directory pair inside the
    // toolchain's `bin` directory, so climbing past the executable name, both
    // of those directories, and `bin` yields the toolchain root -- the same
    // value `VCToolsInstallDir` carries.
    let Some(root) = toolchain_root(tool.path()) else {
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

/// Reads the configured Spectre library directory override for a target.
///
/// The target-specific variable wins over the target-agnostic one. Returns the
/// name of the variable that supplied the value alongside it, so a diagnostic
/// can name the one the integrator actually set. Returns [`None`] when neither
/// is set, which hands the decision to toolchain discovery.
///
/// # Errors
///
/// Returns a human-readable message when a variable is set to a value that is
/// not valid Unicode. A cargo directive is a UTF-8 text line, so such a path
/// could only be emitted lossily -- pointing the linker at a directory that is
/// not the one that was validated. Silently ignoring it would instead fall back
/// to discovery and quietly link the unmitigated CRT.
fn configured_override_dir(override_var: &str) -> Result<Option<(String, PathBuf)>, String> {
    for name in [override_var, GENERIC_OVERRIDE_VAR] {
        match env::var(name) {
            Ok(value) => return Ok(Some((name.to_owned(), PathBuf::from(value)))),
            Err(VarError::NotPresent) => {}
            Err(VarError::NotUnicode(_)) => return Err(unreadable_var_message(name, "used as a library directory")),
        }
    }

    Ok(None)
}

/// Derives the MSVC toolchain root from the path of a compiler executable.
fn toolchain_root(cl_exe: &Path) -> Option<&Path> {
    // `<root>\bin\Host<arch>\<arch>\cl.exe`: drop the file name, the target
    // and host compiler directories, and `bin`.
    cl_exe.parent()?.parent()?.parent()?.parent()
}

/// Emits a `rustc-link-search` directive when `dir` is an existing directory
/// whose path can be written to a cargo directive.
///
/// Returns whether the directive was emitted. A cargo directive is a UTF-8 text
/// line, so a path that is not valid Unicode is rejected rather than
/// lossily converted, which would point the linker at a different directory
/// than the one that was validated.
///
/// The directive is emitted unconditionally for an existing directory: an
/// explicit `-L` search path is consulted before the `LIB` environment variable
/// and in the order given, so suppressing it because the same directory appears
/// somewhere in `LIB` could leave an ordinary CRT directory ahead of the
/// mitigated one. A duplicate search path is harmless to the linker.
fn emit_link_search(dir: &Path) -> bool {
    let Some(text) = dir.to_str() else {
        return false;
    };

    if !dir.is_dir() {
        return false;
    }

    println!("cargo:rustc-link-search=native={text}");
    true
}
