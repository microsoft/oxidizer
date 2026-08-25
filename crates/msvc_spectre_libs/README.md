<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Msvc Spectre Libs Logo" width="96">

# Msvc Spectre Libs

[![crate.io](https://img.shields.io/crates/v/msvc_spectre_libs.svg)](https://crates.io/crates/msvc_spectre_libs)
[![docs.rs](https://docs.rs/msvc_spectre_libs/badge.svg)](https://docs.rs/msvc_spectre_libs)
[![MSRV](https://img.shields.io/crates/msrv/msvc_spectre_libs)](https://crates.io/crates/msvc_spectre_libs)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Link against the Spectre-mitigated MSVC CRT import libraries on Windows.

Adding this crate as a dependency makes its build script add the
Spectre-mitigated (`/Qspectre`) C runtime import libraries to the linker
search path for Windows MSVC targets. Your crate then links against the
hardened runtime automatically, because the `cargo:rustc-link-search`
directive emitted by the build script propagates to every crate that links
this one, up to and including the final artifact. On every
non-Windows-MSVC target the crate does nothing.

## Usage

```toml
[dependencies]
msvc_spectre_libs = "0.2"
```

No source changes are required: linking the Spectre libraries is a pure
build-script side effect. Use a normal `[dependencies]` entry, not
`[build-dependencies]`: a build-dependency lives in the host build graph, so
its link-search path would apply only to that build script rather than to
the shipped target artifact.

### A complete example

A binary crate that must link the mitigated runtime and additionally
requires the shadow-stack linker argument needs three files and no Rust
code changes:

```toml
# Cargo.toml
[package]
name = "hardened_app"
version = "0.1.0"
edition = "2024"

[dependencies]
msvc_spectre_libs = { version = "0.2", features = ["error"] }
```

```toml
# .cargo/config.toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-Clink-arg=/CETCOMPAT"]

[env]
# Fail the build if the linker argument above does not reach rustc.
MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc = "/CETCOMPAT"
```

```rust
// src/main.rs -- unchanged; the crate is a build-time dependency only.
fn main() {
    println!("linked against the Spectre-mitigated CRT");
}
```

Building with `cargo build --target x86_64-pc-windows-msvc` then links the
mitigated import libraries. If the toolchain has no Spectre libraries
installed, or the linker argument does not reach `rustc`, the `error`
feature turns the build diagnostic into a build failure. Confirm the result
on the produced binary with `dumpbin /headers`.

## Locating the libraries

The build script resolves the Spectre library directory from these sources,
in this order:

1. **Build-system override (preferred).** If the environment variable
   `MSVC_SPECTRE_LIB_DIR_<target>` (for example
   `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc`) or the target-agnostic
   `MSVC_SPECTRE_LIB_DIR` points at an existing directory, that directory is
   used verbatim. This lets a build or CI system that already knows the
   toolchain layout (for example one that provisions the MSVC libraries from
   a package feed) supply the exact path without any registry probing.
1. **`VCToolsInstallDir` from a developer environment.** Otherwise, if the
   MSVC build tools export `VCToolsInstallDir` (as a Visual Studio developer
   command prompt, or any shell that has run `vcvars`, does), the script uses
   `lib\spectre\<arch>` directly beneath it.
1. **Registry discovery (fallback).** Otherwise the script locates `cl.exe`
   through the Windows registry and derives the `lib\spectre\<arch>`
   directory that ships with the Visual Studio C++ build tools.

Use [`resolve::override_var_name`][__link0] to compute the target-specific override
variable name, [`resolve::SpectreArch::from_target_arch`][__link1] to map a Rust
target architecture to the matching Spectre architecture, and
[`resolve::spectre_lib_dir`][__link2] to build the `lib\spectre\<arch>` path beneath
a toolchain root.

The resolved directory is always emitted as a search path. An explicit
search path is consulted before the `LIB` environment variable, so emitting
it unconditionally keeps the mitigated libraries ahead of the ordinary ones
even when a build system also lists a CRT directory in `LIB`.

## Required-linker-argument verification

Some hardening arguments cannot be delivered by this crate at all: the
`cargo:rustc-link-arg` directive of a build script applies only to the
artifacts of the package that emits it and does **not** propagate to
dependents, unlike the `cargo:rustc-link-search` used above. Those
arguments must therefore come from `.cargo/config.toml` (or `RUSTFLAGS`)
instead.

Because a `RUSTFLAGS` environment variable *replaces* rather than merges
with `target.<triple>.rustflags` from `.cargo/config.toml`, an unrelated
ambient `RUSTFLAGS` silently drops those arguments. To turn that silent
drop into a build diagnostic, list them in
[`flags::REQUIRED_LINK_ARGS_VAR`][__link3]; the build script then inspects
`CARGO_ENCODED_RUSTFLAGS` and reports any that did not reach `rustc`:

```toml
# .cargo/config.toml
[env]
MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc = "/CETCOMPAT"

[target.x86_64-pc-windows-msvc]
rustflags = ["-Clink-arg=/CETCOMPAT"]
```

The target-suffixed variable takes precedence over the target-agnostic
`MSVC_SPECTRE_REQUIRED_LINK_ARGS`; see
[`flags::required_link_args_var_name`][__link4]. Prefer it whenever a requirement is
architecture-specific, as `/CETCOMPAT` is: the `[env]` table of Cargo
applies to every selected target, so a target-agnostic requirement would be
reported as missing on the targets whose `rustflags` cannot carry it.

The check is off by default: which arguments are required depends on the
toolchain and on the compliance requirements that integrators must meet, so
this crate imposes no policy of its own.

## Assurance boundary

Both steps inspect the *configuration* of a build: whether a library search
path could be resolved, and whether the configured linker arguments appear
in the flags Cargo reports to `rustc`. Neither observes the linker
invocation, and neither inspects the produced binary. A successful build –
including with the `error` feature enabled – therefore establishes that the
build was configured to link the mitigated runtime, not that the artifact
provably did. Treat it as a configuration gate, and use post-link
inspection such as `dumpbin /headers` where artifact-level evidence is
required.

## Features

* `error`: turn build diagnostics (Spectre libraries not found, or a
  required linker argument missing from `rustc`) into hard build errors, for
  builds that must not silently proceed with an incomplete hardening
  configuration.

## Further reading

[`docs/design.md`][__link5]
records the user-visible contract and the design tenets, and
[`docs/implementation.md`][__link6]
records the implementation strategy: the host/target model, the discovery
flow, shared source, flag parsing, path handling, emission, and diagnostics.

## Examples

```rust
use msvc_spectre_libs::resolve::{SpectreArch, override_var_name};

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
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/msvc_spectre_libs">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbX2HZIX02W2YbrJHS1Iop1u8b9R7A5DFDAJIbc-_L20PJHthhZIGCcW1zdmNfc3BlY3RyZV9saWJzZTAuMi4w
 [__link0]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::override_var_name
 [__link1]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::SpectreArch::from_target_arch
 [__link2]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::spectre_lib_dir
 [__link3]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=flags::REQUIRED_LINK_ARGS_VAR
 [__link4]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=flags::required_link_args_var_name
 [__link5]: https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/design.md
 [__link6]: https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/implementation.md
