// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]
#![doc(html_favicon_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]

//! Link against the Spectre-mitigated MSVC CRT import libraries on Windows.
//!
//! Adding this crate as a dependency makes its build script add the
//! Spectre-mitigated (`/Qspectre`) C runtime import libraries to the linker
//! search path for Windows MSVC targets. Your crate then links against the
//! hardened runtime automatically, because the `cargo:rustc-link-search`
//! directive emitted by the build script propagates to every crate that links
//! this one, up to and including the final artifact. On every
//! non-Windows-MSVC target the crate does nothing.
//!
//! # Usage
//!
//! ```toml
//! [dependencies]
//! msvc_spectre_libs = "0.2"
//! ```
//!
//! No source changes are required: linking the Spectre libraries is a pure
//! build-script side effect. Use a normal `[dependencies]` entry, not
//! `[build-dependencies]`: a build-dependency lives in the host build graph, so
//! its link-search path would apply only to that build script rather than to
//! the shipped target artifact.
//!
//! ## A complete example
//!
//! A binary crate that must link the mitigated runtime and additionally
//! requires the shadow-stack linker argument needs three files and no Rust
//! code changes:
//!
//! ```toml
//! # Cargo.toml
//! [package]
//! name = "hardened_app"
//! version = "0.1.0"
//! edition = "2024"
//!
//! [dependencies]
//! msvc_spectre_libs = { version = "0.2", features = ["error"] }
//! ```
//!
//! ```toml
//! # .cargo/config.toml
//! [target.x86_64-pc-windows-msvc]
//! rustflags = ["-Clink-arg=/CETCOMPAT"]
//!
//! [env]
//! # Fail the build if the linker argument above does not reach rustc.
//! MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc = "/CETCOMPAT"
//! ```
//!
//! ```rust,ignore
//! // src/main.rs -- unchanged; the crate is a build-time dependency only.
//! fn main() {
//!     println!("linked against the Spectre-mitigated CRT");
//! }
//! ```
//!
//! Building with `cargo build --target x86_64-pc-windows-msvc` then links the
//! mitigated import libraries. If the toolchain has no Spectre libraries
//! installed, or the linker argument does not reach `rustc`, the `error`
//! feature turns the build diagnostic into a build failure. Confirm the result
//! on the produced binary with `dumpbin /headers`.
//!
//! # Locating the libraries
//!
//! The build script resolves the Spectre library directory from these sources,
//! in this order:
//!
//! 1. **Build-system override (preferred).** If the environment variable
//!    `MSVC_SPECTRE_LIB_DIR_<target>` (for example
//!    `MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc`) or the target-agnostic
//!    `MSVC_SPECTRE_LIB_DIR` points at an existing directory, that directory is
//!    used verbatim. This lets a build or CI system that already knows the
//!    toolchain layout (for example one that provisions the MSVC libraries from
//!    a package feed) supply the exact path without any registry probing.
//! 2. **`VCToolsInstallDir` from a developer environment.** Otherwise, if the
//!    MSVC build tools export `VCToolsInstallDir` (as a Visual Studio developer
//!    command prompt, or any shell that has run `vcvars`, does), the script uses
//!    `lib\spectre\<arch>` directly beneath it.
//! 3. **Registry discovery (fallback).** Otherwise the script locates `cl.exe`
//!    through the Windows registry and derives the `lib\spectre\<arch>`
//!    directory that ships with the Visual Studio C++ build tools.
//!
//! Use [`resolve::override_var_name`] to compute the target-specific override
//! variable name, [`resolve::SpectreArch::from_target_arch`] to map a Rust
//! target architecture to the matching Spectre architecture, and
//! [`resolve::spectre_lib_dir`] to build the `lib\spectre\<arch>` path beneath
//! a toolchain root.
//!
//! The resolved directory is always emitted as a search path. An explicit
//! search path is consulted before the `LIB` environment variable, so emitting
//! it unconditionally keeps the mitigated libraries ahead of the ordinary ones
//! even when a build system also lists a CRT directory in `LIB`.
//!
//! # Required-linker-argument verification
//!
//! Some hardening arguments cannot be delivered by this crate at all: the
//! `cargo:rustc-link-arg` directive of a build script applies only to the
//! artifacts of the package that emits it and does **not** propagate to
//! dependents, unlike the `cargo:rustc-link-search` used above. Those
//! arguments must therefore come from `.cargo/config.toml` (or `RUSTFLAGS`)
//! instead.
//!
//! Because a `RUSTFLAGS` environment variable *replaces* rather than merges
//! with `target.<triple>.rustflags` from `.cargo/config.toml`, an unrelated
//! ambient `RUSTFLAGS` silently drops those arguments. To turn that silent
//! drop into a build diagnostic, list them in
//! [`flags::REQUIRED_LINK_ARGS_VAR`]; the build script then inspects
//! `CARGO_ENCODED_RUSTFLAGS` and reports any that did not reach `rustc`:
//!
//! ```toml
//! # .cargo/config.toml
//! [env]
//! MSVC_SPECTRE_REQUIRED_LINK_ARGS_x86_64_pc_windows_msvc = "/CETCOMPAT"
//!
//! [target.x86_64-pc-windows-msvc]
//! rustflags = ["-Clink-arg=/CETCOMPAT"]
//! ```
//!
//! The target-suffixed variable takes precedence over the target-agnostic
//! `MSVC_SPECTRE_REQUIRED_LINK_ARGS`; see
//! [`flags::required_link_args_var_name`]. Prefer it whenever a requirement is
//! architecture-specific, as `/CETCOMPAT` is: the `[env]` table of Cargo
//! applies to every selected target, so a target-agnostic requirement would be
//! reported as missing on the targets whose `rustflags` cannot carry it.
//!
//! The check is off by default: which arguments are required depends on the
//! toolchain and on the compliance requirements that integrators must meet, so
//! this crate imposes no policy of its own.
//!
//! # Assurance boundary
//!
//! Both steps inspect the *configuration* of a build: whether a library search
//! path could be resolved, and whether the configured linker arguments appear
//! in the flags Cargo reports to `rustc`. Neither observes the linker
//! invocation, and neither inspects the produced binary. A successful build --
//! including with the `error` feature enabled -- therefore establishes that the
//! build was configured to link the mitigated runtime, not that the artifact
//! provably did. Treat it as a configuration gate, and use post-link
//! inspection such as `dumpbin /headers` where artifact-level evidence is
//! required.
//!
//! # Features
//!
//! - `error`: turn build diagnostics (Spectre libraries not found, or a
//!   required linker argument missing from `rustc`) into hard build errors, for
//!   builds that must not silently proceed with an incomplete hardening
//!   configuration.
//!
//! # Further reading
//!
//! [`docs/design.md`](https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/design.md)
//! records the user-visible contract and the design tenets, and
//! [`docs/implementation.md`](https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/implementation.md)
//! records the implementation strategy: the host/target model, the discovery
//! flow, shared source, flag parsing, path handling, emission, and diagnostics.
//!
//! # Examples
//!
//! ```
//! use msvc_spectre_libs::resolve::{SpectreArch, override_var_name};
//!
//! assert_eq!(
//!     override_var_name("x86_64-pc-windows-msvc"),
//!     "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc"
//! );
//! assert_eq!(SpectreArch::from_target_arch("x86_64"), Some(SpectreArch::X64));
//! assert_eq!(SpectreArch::from_target_arch("aarch64"), Some(SpectreArch::Arm64));
//! assert_eq!(SpectreArch::from_target_arch("riscv64"), None);
//! ```

pub mod flags;
pub mod resolve;
