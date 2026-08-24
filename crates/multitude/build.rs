// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Detects codegen backends that cap type alignment below what the tests need.
//!
//! Not every codegen backend accepts the maximum type alignment the reference
//! implementation does. Several tests deliberately construct types aligned to
//! half a chunk to exercise the arena's alignment handling, well above the cap
//! such a backend imposes, so those tests cannot be compiled there at all. They
//! are gated on `cfg(align_capped_backend)`.
//!
//! There is no built-in cfg identifying the codegen backend
//! (<https://developercommunity.visualstudio.com/t/Conditional-compilation-lacks-built-in-c/11107823>),
//! so the capability is probed directly rather than inferred from the toolchain
//! name. Probing the limit that actually matters keeps the gate correct if a
//! backend raises its cap, or if another backend adopts the same restriction.
//!
//! A second probe at a benign alignment acts as a control. Without it, any
//! `rustc` failure unrelated to alignment — a target whose `core` is not
//! installed, a custom target spec, an unusable sysroot — would be
//! misattributed to the alignment cap and would silently disable the gated
//! tests. The cfg is set only when the control compiles and the over-aligned
//! probe does not.

use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs};

/// Alignment the probe requests, in bytes.
///
/// This is the largest alignment any gated test relies on (`HugeAlign` and
/// `HugeAlignBox` in `tests/arena.rs`). It must stay in sync with the
/// `#[repr(align(...))]` values guarded by `cfg(align_capped_backend)`; probing a
/// smaller value would leave the gate unset on a backend that accepts the
/// probe but still rejects a larger gated type.
const PROBE_ALIGN: u32 = 131_072;

/// Alignment used by the control probe, in bytes.
///
/// Any backend can satisfy this, so a control failure means the probe could
/// not be compiled here for reasons unrelated to alignment.
const CONTROL_ALIGN: u32 = 8;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=RUSTC");
    println!("cargo::rerun-if-env-changed=RUSTUP_TOOLCHAIN");

    if !supports_probe_alignment() {
        println!("cargo::rustc-cfg=align_capped_backend");
    }
}

/// Reports whether codegen accepts a type requiring [`PROBE_ALIGN`].
///
/// Returns `true` whenever the answer cannot be established, so an unexpected
/// environment leaves the tests enabled rather than silently skipping them.
fn supports_probe_alignment() -> bool {
    let Some(out_dir) = env::var_os("OUT_DIR") else {
        return true;
    };
    let out_dir = Path::new(&out_dir);

    // Only a control that compiles makes a failure of the real probe
    // attributable to the alignment cap rather than to the environment.
    if compile_probe(out_dir, CONTROL_ALIGN) != Some(true) {
        return true;
    }

    compile_probe(out_dir, PROBE_ALIGN).unwrap_or(true)
}

/// Compiles a crate containing a type aligned to `align`.
///
/// Returns `Some(true)` when codegen accepted it, `Some(false)` when `rustc`
/// ran and rejected it, and `None` when the probe could not be run at all.
fn compile_probe(out_dir: &Path, align: u32) -> Option<bool> {
    let source_path = out_dir.join(format!("align_probe_{align}.rs"));
    let object_path = out_dir.join(format!("align_probe_{align}.o"));

    // The probe must *materialize* a place of the over-aligned type: backends
    // that cap alignment reject it when laying out a value, not when the type
    // merely appears in a reference parameter, so a slice-taking function
    // compiles cleanly even on a backend that rejects the gated tests.
    // Returning the value by value is enough, and keeping `Probe` a ZST avoids
    // an over-aligned stack frame. An over-aligned `static` is deliberately not
    // used — that hits the COFF section-alignment limit and crashes stock
    // `rustc`, which would set the cfg on every Windows host.
    // No attributes beyond `repr`/`inline` are used, so the source stays valid
    // regardless of which edition `rustc` defaults to.
    let source = format!(
        "#![no_std]\n\
         #[repr(align({align}))]\n\
         pub struct Probe;\n\
         #[inline(never)]\n\
         pub fn probe() -> Probe {{ Probe }}\n"
    );

    fs::write(&source_path, source).ok()?;

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

    Some(command.status().ok()?.success())
}
