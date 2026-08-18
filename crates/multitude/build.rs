// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Detects codegen backends that cap type alignment below what the tests need.
//!
//! The UTC backend used by the internal `ms-prod` toolchains accepts at most
//! 8192-byte alignment, while several tests deliberately construct types aligned
//! to half a chunk to exercise the arena's alignment handling. Those tests are
//! gated on `cfg(utc_backend)`.
//!
//! There is no built-in cfg identifying the codegen backend
//! (<https://developercommunity.visualstudio.com/t/Conditional-compilation-lacks-built-in-c/11107823>),
//! so the capability is probed directly rather than inferred from the toolchain
//! name. Probing the limit that actually matters keeps the gate correct if a
//! backend raises its cap, or if another backend adopts the same restriction.

use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs};

/// Alignment the probe requests, in bytes.
///
/// This is the largest alignment any gated test relies on. It must stay in sync
/// with the `#[repr(align(...))]` values guarded by `cfg(utc_backend)`.
const PROBE_ALIGN: u32 = 32768;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=RUSTC");
    println!("cargo::rerun-if-env-changed=RUSTUP_TOOLCHAIN");

    if !supports_probe_alignment() {
        println!("cargo::rustc-cfg=utc_backend");
    }
}

/// Compiles a type requiring [`PROBE_ALIGN`] and reports whether codegen accepted it.
///
/// Returns `true` when the probe cannot be run at all, so an unexpected
/// environment leaves the tests enabled rather than silently skipping them.
fn supports_probe_alignment() -> bool {
    let Some(out_dir) = env::var_os("OUT_DIR") else {
        return true;
    };

    let source_path = Path::new(&out_dir).join("align_probe.rs");
    let object_path = Path::new(&out_dir).join("align_probe.o");

    // The probe mirrors the shape the gated tests use: a slice of an
    // over-aligned type driven through codegen by a function that cannot be
    // optimized away. An over-aligned `static` is deliberately not used — that
    // hits the COFF section-alignment limit and crashes stock `rustc`, which
    // would make the probe report a false positive on every Windows host.
    // No attributes beyond `repr`/`inline` are used, so the source stays valid
    // regardless of which edition `rustc` defaults to.
    let source = format!(
        "#![no_std]\n\
         #[repr(align({PROBE_ALIGN}))]\n\
         pub struct Probe;\n\
         #[inline(never)]\n\
         pub fn probe(values: &[Probe]) -> usize {{ values.len() }}\n"
    );

    if fs::write(&source_path, source).is_err() {
        return true;
    }

    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut command = Command::new(rustc);
    command
        .arg("--crate-type=lib")
        .arg("--emit=obj")
        .arg("-o")
        .arg(&object_path)
        .arg(&source_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Build scripts run on the host, but the cfg applies to the target being
    // compiled, so the probe has to target it too.
    if let Some(target) = env::var_os("TARGET") {
        command.arg("--target").arg(target);
    }

    command.status().is_ok_and(|status| status.success())
}
