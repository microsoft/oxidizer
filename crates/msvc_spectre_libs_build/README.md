<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Msvc Spectre Libs Build Logo" width="96">

# Msvc Spectre Libs Build

[![crate.io](https://img.shields.io/crates/v/msvc_spectre_libs_build.svg)](https://crates.io/crates/msvc_spectre_libs_build)
[![docs.rs](https://docs.rs/msvc_spectre_libs_build/badge.svg)](https://docs.rs/msvc_spectre_libs_build)
[![MSRV](https://img.shields.io/crates/msrv/msvc_spectre_libs_build)](https://crates.io/crates/msvc_spectre_libs_build)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Build-time policy behind [`msvc_spectre_libs`][__link0].

[`msvc_spectre_libs`][__link1] is a build-script-only crate: adding it as a
dependency makes the Spectre-mitigated (`/Qspectre`) MSVC CRT import
libraries reachable from the linker, and optionally verifies that the
hardening linker arguments it cannot propagate itself actually reached
`rustc`. This crate is where the decision behind that lives, packaged
separately so it has exactly one compilation owner: `msvc_spectre_libs`
consumes it through `[build-dependencies]` rather than by including its
source twice.

Depend on this crate directly only to reuse the policy or the naming
helpers – for example from another build script, or from a build system
that computes the override variable names itself. To simply link the
mitigated runtime, depend on [`msvc_spectre_libs`][__link2] instead.

## Structure

* [`plan`][__link3] is the decision: [`plan::plan`][__link4] maps a captured
  [`plan::BuildEnvironment`][__link5] plus a [`toolchain::Toolchain`][__link6] to the
  [`plan::Plan`][__link7] a build script should carry out. It reads no process
  environment, prints nothing, and never exits.
* [`toolchain`][__link8] names the two machine-dependent questions the decision
  needs answered – does this directory exist, and where is `cl.exe` –
  so that the decision can be exercised without an installed toolchain.
* [`resolve`][__link9] holds the naming and path helpers: override variable names,
  the target-architecture mapping, and the `lib\spectre\<arch>` layout.
* [`flags`][__link10] holds the flag-verification helpers: the required-argument
  variable names, and the decoding of the flags Cargo reports to `rustc`.

## Usage

A build script is the adapter around [`plan::plan`][__link11]: it captures the
environment, asks for a plan, and prints it.

```rust
// build.rs
use msvc_spectre_libs_build::plan::{BuildEnvironment, Plan, plan};
use msvc_spectre_libs_build::toolchain::SystemToolchain;

fn main() {
    let plan = BuildEnvironment::from_env()
        .map_or_else(Plan::reporting, |environment| plan(&environment, &SystemToolchain));

    for name in &plan.rerun_if_env_changed {
        println!("cargo:rerun-if-env-changed={name}");
    }
    for dir in &plan.link_search {
        println!("cargo:rustc-link-search=native={dir}");
    }
    for warning in plan.warnings() {
        println!("cargo:warning={warning}");
    }
}
```

## Assurance boundary

The policy inspects the *configuration* of a build: whether a library search
path could be resolved, and whether the configured linker arguments appear
in the flags Cargo reports to `rustc`. It does not observe the linker
invocation and does not inspect the produced artifact, so a plan without
diagnostics establishes that the build was configured to link the mitigated
runtime, not that the artifact provably did. Post-link inspection, for
example `dumpbin /headers`, remains the only artifact-level evidence.

## Further reading

[`docs/design.md`][__link12]
records the user-visible contract and the design tenets, and
[`docs/implementation.md`][__link13]
records the implementation strategy: the host/target model, the discovery
flow, flag parsing, path handling, emission, and diagnostics.

## Examples

```rust
use msvc_spectre_libs_build::resolve::{SpectreArch, override_var_name};

assert_eq!(
    override_var_name("x86_64-pc-windows-msvc"),
    "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc"
);
assert_eq!(
    SpectreArch::from_target_arch("x86_64"),
    Some(SpectreArch::X64)
);
assert_eq!(
    SpectreArch::from_target_arch("aarch64"),
    Some(SpectreArch::Arm64)
);
assert_eq!(SpectreArch::from_target_arch("riscv64"), None);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/msvc_spectre_libs_build">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbC52U2RKb8ycb6eQqnl9haVYb52EFsO7a-swbt-X_H4uXCoJhZIGCd21zdmNfc3BlY3RyZV9saWJzX2J1aWxkZTAuMS4w
 [__link0]: https://docs.rs/msvc_spectre_libs
 [__link1]: https://docs.rs/msvc_spectre_libs
 [__link10]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/flags/index.html
 [__link11]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/?search=plan::plan
 [__link12]: https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/design.md
 [__link13]: https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/implementation.md
 [__link2]: https://docs.rs/msvc_spectre_libs
 [__link3]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/plan/index.html
 [__link4]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/?search=plan::plan
 [__link5]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/?search=plan::BuildEnvironment
 [__link6]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/?search=toolchain::Toolchain
 [__link7]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/?search=plan::Plan
 [__link8]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/toolchain/index.html
 [__link9]: https://docs.rs/msvc_spectre_libs_build/0.1.0/msvc_spectre_libs_build/resolve/index.html
