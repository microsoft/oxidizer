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

## Locating the libraries

The build script resolves the Spectre library directory in three steps:

1. **Build-system override (preferred).** If the environment variable
   `MSVC_SPECTRE_LIB_DIR_<target>` (for example
   `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc`) or the target-agnostic
   `MSVC_SPECTRE_LIB_DIR` points at an existing directory, that directory is
   used verbatim. This lets an enlistment or CI system that already knows the
   toolchain layout (for example one that provisions the MSVC libraries from
   a package feed) supply the exact path without any registry probing.
1. **Enlistment toolchain (`VCToolsInstallDir`).** Otherwise, if the MSVC
   build tools export `VCToolsInstallDir` (as a Visual Studio developer
   command prompt or an enlistment that runs `vcvars` does), the script uses
   `lib\spectre\<arch>` directly beneath it.
1. **Registry discovery (fallback).** Otherwise the script locates `cl.exe`
   through the Windows registry and derives the `lib\spectre\<arch>`
   directory that ships with the Visual Studio C++ build tools.

Use [`resolve::override_var_name`][__link0] to compute the target-specific override
variable name, [`resolve::spectre_arch`][__link1] to map a Rust target architecture
to the matching Spectre subdirectory, and [`resolve::spectre_lib_dir`][__link2] to
build the `lib\spectre\<arch>` path beneath a toolchain root.

## Idempotence

The build script never adds a search path that is already in effect. Before
emitting anything it checks whether the resolved directory is already
supplied through `RUSTFLAGS` (as `-L native=<dir>`) or through the `LIB`
environment variable that `link.exe` reads directly, and stays silent if so.
Emitting the same directory twice would be harmless to the linker but hides
a misconfigured build system, so it is reported as a skip instead.

## Required-flag verification

Some hardening flags cannot be delivered by this crate at all: the
`cargo:rustc-link-arg` directive of a build script applies only to the
artifacts of the package that emits it and does **not** propagate to
dependents, unlike the `cargo:rustc-link-search` used above. Those flags
must therefore come from `.cargo/config.toml` (or `RUSTFLAGS`) instead.

Because a `RUSTFLAGS` environment variable *replaces* rather than merges
with `target.<triple>.rustflags` from `.cargo/config.toml`, an unrelated
ambient `RUSTFLAGS` silently drops those flags. To make that failure loud,
list them in [`flags::REQUIRED_LINK_ARGS_VAR`][__link3] and the build script will
inspect `CARGO_ENCODED_RUSTFLAGS` and report any that did not reach `rustc`:

```toml
# .cargo/config.toml
[env]
MSVC_SPECTRE_REQUIRED_LINK_ARGS = "/CETCOMPAT"

[target.x86_64-pc-windows-msvc]
rustflags = ["-Clink-arg=/CETCOMPAT"]
```

The check is off by default: which arguments are required depends on the
toolchain and on the compliance bar that integrators must meet, so this
crate imposes no policy of its own.

## Features

* `error`: turn build warnings (Spectre libraries not found, or a required
  hardening flag missing from `rustc`) into hard build errors, for builds
  that must not silently ship an unmitigated binary.

## Examples

```rust
use msvc_spectre_libs::resolve::{override_var_name, spectre_arch};

assert_eq!(
    override_var_name("x86_64-pc-windows-msvc"),
    "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc"
);
assert_eq!(spectre_arch("x86_64"), Some("x64"));
assert_eq!(spectre_arch("aarch64"), Some("arm64"));
assert_eq!(spectre_arch("riscv64"), None);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/msvc_spectre_libs">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbp47aYEZyz1gboSE8o1l280UbNBUfibw9RiUbiuDUw_AzJpphZIGCcW1zdmNfc3BlY3RyZV9saWJzZTAuMi4w
 [__link0]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::override_var_name
 [__link1]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::spectre_arch
 [__link2]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::spectre_lib_dir
 [__link3]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=flags::REQUIRED_LINK_ARGS_VAR
