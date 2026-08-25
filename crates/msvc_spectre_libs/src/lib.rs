// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]
#![doc(html_favicon_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]

//! Link against the Spectre-mitigated MSVC CRT import libraries on Windows.
//!
//! Adding this crate as a build dependency makes its build script add the
//! Spectre-mitigated (`/Qspectre`) C runtime import libraries to the linker
//! search path for Windows MSVC targets. Dependent crates then link against the
//! hardened runtime automatically, because a `cargo:rustc-link-search`
//! directive propagates from a dependency to every crate that depends on it. On
//! every non-Windows-MSVC target the crate does nothing.
//!
//! # Usage
//!
//! ```toml
//! [build-dependencies]
//! msvc_spectre_libs = "0.2"
//! ```
//!
//! No source changes are required: linking the Spectre libraries is a pure
//! build-script side effect.
//!
//! # Locating the libraries
//!
//! The build script resolves the Spectre library directory in two steps:
//!
//! 1. **Build-system override (preferred).** If the environment variable
//!    `MSVC_SPECTRE_LIB_DIR_<target>` (for example
//!    `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc`) or the target-agnostic
//!    `MSVC_SPECTRE_LIB_DIR` points at an existing directory, that directory is
//!    used verbatim. This lets an enlistment or CI system that already knows the
//!    toolchain layout (for example one that provisions the MSVC libraries from
//!    a package feed) supply the exact path without any registry probing.
//! 2. **Toolchain discovery (fallback).** Otherwise the script locates `cl.exe`
//!    through the Windows registry and derives the `lib\spectre\<arch>`
//!    directory that ships with the Visual Studio C++ build tools.
//!
//! Use [`resolve::override_var_name`] to compute the target-specific override
//! variable name and [`resolve::spectre_arch`] to map a Rust target
//! architecture to the toolchain's Spectre subdirectory.
//!
//! # Features
//!
//! - `error`: turn the "libraries not found" build warning into a hard build
//!   error, for builds that must not silently fall back to the unmitigated
//!   runtime.
//!
//! # Examples
//!
//! ```
//! use msvc_spectre_libs::resolve::{override_var_name, spectre_arch};
//!
//! assert_eq!(override_var_name("x86_64-pc-windows-msvc"), "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc");
//! assert_eq!(spectre_arch("x86_64"), Some("x64"));
//! assert_eq!(spectre_arch("aarch64"), Some("arm64"));
//! assert_eq!(spectre_arch("riscv64"), None);
//! ```

pub mod resolve;
