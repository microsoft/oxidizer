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

Adding this crate as a build dependency makes its build script add the
Spectre-mitigated (`/Qspectre`) C runtime import libraries to the linker
search path for Windows MSVC targets. Dependent crates then link against the
hardened runtime automatically, because a `cargo:rustc-link-search`
directive propagates from a dependency to every crate that depends on it. On
every non-Windows-MSVC target the crate does nothing.

## Usage

```toml
[build-dependencies]
msvc_spectre_libs = "0.2"
```

No source changes are required: linking the Spectre libraries is a pure
build-script side effect.

## Locating the libraries

The build script resolves the Spectre library directory in two steps:

1. **Build-system override (preferred).** If the environment variable
   `MSVC_SPECTRE_LIB_DIR_<target>` (for example
   `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc`) or the target-agnostic
   `MSVC_SPECTRE_LIB_DIR` points at an existing directory, that directory is
   used verbatim. This lets an enlistment or CI system that already knows the
   toolchain layout (for example one that provisions the MSVC libraries from
   a package feed) supply the exact path without any registry probing.
1. **Toolchain discovery (fallback).** Otherwise the script locates `cl.exe`
   through the Windows registry and derives the `lib\spectre\<arch>`
   directory that ships with the Visual Studio C++ build tools.

Use [`resolve::override_var_name`][__link0] to compute the target-specific override
variable name and [`resolve::spectre_arch`][__link1] to map a Rust target
architecture to the toolchain’s Spectre subdirectory.

## Features

* `error`: turn the “libraries not found” build warning into a hard build
  error, for builds that must not silently fall back to the unmitigated
  runtime.

## Examples

```rust
use msvc_spectre_libs::resolve::{override_var_name, spectre_arch};

assert_eq!(override_var_name("x86_64-pc-windows-msvc"), "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc");
assert_eq!(spectre_arch("x86_64"), Some("x64"));
assert_eq!(spectre_arch("aarch64"), Some("arm64"));
assert_eq!(spectre_arch("riscv64"), None);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/msvc_spectre_libs">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbjFOs0DqVlxAbT_w4_2ENvfwb02w1X_d2lz8bQiyrGUcXbq1hZIGCcW1zdmNfc3BlY3RyZV9saWJzZTAuMi4w
 [__link0]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::override_var_name
 [__link1]: https://docs.rs/msvc_spectre_libs/0.2.0/msvc_spectre_libs/?search=resolve::spectre_arch
