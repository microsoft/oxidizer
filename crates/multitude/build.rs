// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Detects codegen backends that cap type alignment below what the tests need.
//!
//! Some codegen backends cap the maximum type alignment they can lay out.
//! Several tests deliberately construct types aligned to 32 KiB, 64 KiB and
//! 128 KiB to exercise the arena's alignment guards, whose thresholds sit above
//! any such cap, so on a capped backend those tests cannot be compiled at all.
//! They are gated on `cfg(align_capped_backend)`.
//!
//! The gate is a single boolean driven by the largest of those alignments, so a
//! backend that rejects it disables every gated test, including ones needing
//! only 32 KiB. That is deliberate: the guards under test are unreachable
//! without an over-aligned type in the first place, so partial coverage would
//! not be meaningful.
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

/// Largest alignment `#[repr(align(...))]` accepts, in bytes.
///
/// <https://doc.rust-lang.org/reference/type-layout.html#the-align-and-packed-modifiers>
const MAX_REPR_ALIGN: u32 = 1 << 29;

fn main() {
    // Declaring the cfg here covers every unit it can reach, so the workspace
    // `check-cfg` allowlist does not need a crate-local entry for it.
    println!("cargo::rustc-check-cfg=cfg(align_capped_backend)");

    println!("cargo::rerun-if-changed=build.rs");
    // `RUSTC` alone does not pin the compiler: a rustup proxy keeps the same
    // command while `RUSTUP_TOOLCHAIN` selects a different toolchain behind it,
    // and `RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` can swap the codegen backend
    // outright via `-Zcodegen-backend`. Cargo caches this script's output, so
    // without these triggers a stale capability result would survive a
    // toolchain or backend switch and gate the tests against the wrong compiler.
    println!("cargo::rerun-if-env-changed=RUSTC");
    println!("cargo::rerun-if-env-changed=RUSTUP_TOOLCHAIN");
    println!("cargo::rerun-if-env-changed=RUSTFLAGS");
    println!("cargo::rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");

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

    // Only a control that compiles makes a failure of the over-aligned probe
    // attributable to the alignment cap rather than to the environment.
    if compile_probe(out_dir, CONTROL_ALIGN) != Some(true) {
        return true;
    }

    compile_probe(out_dir, PROBE_ALIGN).unwrap_or(true)
}

/// Flags Cargo is passing to `rustc` for this build, if any.
///
/// The probe has to be compiled the same way the crate is, or it would answer
/// for a different compiler configuration than the one the tests will use:
/// `-Zcodegen-backend` in particular selects the very thing being detected.
/// `CARGO_ENCODED_RUSTFLAGS` is authoritative and unambiguous when set, since
/// it is `\x1f`-separated and so survives flags containing spaces.
///
/// Forwarding is safe in both directions. Flags that make compilation fail for
/// unrelated reasons fail the control probe first, which leaves the tests
/// enabled, and flags that break only the over-aligned probe are an alignment
/// cap by definition.
fn rustc_flags() -> Vec<String> {
    if let Some(encoded) = env::var_os("CARGO_ENCODED_RUSTFLAGS") {
        return encoded
            .to_string_lossy()
            .split('\x1f')
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect();
    }
    env::var_os("RUSTFLAGS")
        .map(|flags| flags.to_string_lossy().split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Compiles a crate containing a type aligned to `align`.
///
/// Returns `Some(true)` when codegen accepted it, `Some(false)` when `rustc`
/// ran and rejected it, and `None` when the probe could not be run at all.
fn compile_probe(out_dir: &Path, align: u32) -> Option<bool> {
    // An alignment `repr(align(...))` cannot express is rejected by `rustc` for
    // a source-language reason, which the caller would then misread as a
    // backend cap and use to disable the tests everywhere. Fail at the bad
    // constant instead of emitting a capability result that is not one.
    assert!(
        align.is_power_of_two() && align <= MAX_REPR_ALIGN,
        "probe alignment {align} must be a power of two no greater than {MAX_REPR_ALIGN}"
    );

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
    // `probe` is public because public reachability is what keeps it alive: a
    // private, uncalled function can be discarded before codegen, and then
    // nothing would exercise the layout. No attributes beyond `repr` are used,
    // so the source stays valid regardless of which edition `rustc` defaults to.
    let source = format!(
        "#![no_std]\n\
         #[repr(align({align}))]\n\
         pub struct Probe;\n\
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
        .args(rustc_flags())
        .arg(&source_path)
        // Both streams are discarded because a rejection here is an expected
        // capability result, not a fault: surfacing it would print an alarming
        // compiler error in the middle of an otherwise successful build. The
        // control probe is what recovers the information this hides, by
        // establishing that `rustc` works at all before any failure of the
        // over-aligned probe is attributed to the alignment cap.
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Build scripts run on the host, but the cfg applies to the target being
    // compiled, so the probe has to target it too.
    if let Some(target) = env::var_os("TARGET") {
        command.arg("--target").arg(target);
    }

    Some(command.status().ok()?.success())
}
