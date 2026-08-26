// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]
#![doc(html_favicon_url = "https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg")]

//! Build-time policy behind [`msvc_spectre_libs`].
//!
//! [`msvc_spectre_libs`] is a build-script-only crate: adding it as a
//! dependency makes the Spectre-mitigated (`/Qspectre`) MSVC CRT import
//! libraries reachable from the linker, and optionally verifies that the
//! hardening linker arguments it cannot propagate itself actually reached
//! `rustc`. This crate is where the decision behind that lives, packaged
//! separately so it has exactly one compilation owner: `msvc_spectre_libs`
//! consumes it through `[build-dependencies]` rather than by including its
//! source twice.
//!
//! Depend on this crate directly only to reuse the policy or the naming
//! helpers -- for example from another build script, or from a build system
//! that computes the override variable names itself. To simply link the
//! mitigated runtime, depend on [`msvc_spectre_libs`] instead.
//!
//! # Structure
//!
//! - [`plan`] is the decision: [`plan::plan`] maps a captured
//!   [`plan::BuildEnvironment`] plus a [`toolchain::Toolchain`] to the
//!   [`plan::Plan`] a build script should carry out. It reads no process
//!   environment, prints nothing, and never exits.
//! - [`toolchain`] names the two machine-dependent questions the decision
//!   needs answered -- does this directory exist, and where is `cl.exe` --
//!   so that the decision can be exercised without an installed toolchain.
//! - [`resolve`] holds the naming and path helpers: override variable names,
//!   the target-architecture mapping, and the `lib\spectre\<arch>` layout.
//! - [`flags`] holds the flag-verification helpers: the required-argument
//!   variable names, and the decoding of the flags Cargo reports to `rustc`.
//!
//! # Usage
//!
//! A build script is the adapter around [`plan::plan`]: it captures the
//! environment, asks for a plan, and prints it.
//!
//! ```rust,ignore
//! // build.rs
//! use msvc_spectre_libs_build::plan::{BuildEnvironment, Plan, plan};
//! use msvc_spectre_libs_build::toolchain::SystemToolchain;
//!
//! fn main() {
//!     let plan = BuildEnvironment::from_env()
//!         .map_or_else(Plan::reporting, |environment| plan(&environment, &SystemToolchain));
//!
//!     for name in &plan.rerun_if_env_changed {
//!         println!("cargo:rerun-if-env-changed={name}");
//!     }
//!     for dir in &plan.link_search {
//!         println!("cargo:rustc-link-search=native={dir}");
//!     }
//!     for diagnostic in &plan.diagnostics {
//!         println!("cargo:warning={diagnostic}");
//!     }
//! }
//! ```
//!
//! # Assurance boundary
//!
//! The policy inspects the *configuration* of a build: whether a library search
//! path could be resolved, and whether the configured linker arguments appear
//! in the flags Cargo reports to `rustc`. It does not observe the linker
//! invocation and does not inspect the produced artifact, so a plan without
//! diagnostics establishes that the build was configured to link the mitigated
//! runtime, not that the artifact provably did. Post-link inspection, for
//! example `dumpbin /headers`, remains the only artifact-level evidence.
//!
//! # Further reading
//!
//! [`docs/design.md`](https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/design.md)
//! records the user-visible contract and the design tenets, and
//! [`docs/implementation.md`](https://github.com/microsoft/oxidizer/blob/main/crates/msvc_spectre_libs/docs/implementation.md)
//! records the implementation strategy: the host/target model, the discovery
//! flow, flag parsing, path handling, emission, and diagnostics.
//!
//! [`msvc_spectre_libs`]: https://docs.rs/msvc_spectre_libs
//!
//! # Examples
//!
//! ```
//! use msvc_spectre_libs_build::resolve::{SpectreArch, override_var_name};
//!
//! assert_eq!(
//!     override_var_name("x86_64-pc-windows-msvc"),
//!     "MSVC_SPECTRE_LIB_DIR_x86_64_pc_windows_msvc"
//! );
//! assert_eq!(
//!     SpectreArch::from_target_arch("x86_64"),
//!     Some(SpectreArch::X64)
//! );
//! assert_eq!(
//!     SpectreArch::from_target_arch("aarch64"),
//!     Some(SpectreArch::Arm64)
//! );
//! assert_eq!(SpectreArch::from_target_arch("riscv64"), None);
//! ```

pub mod flags;
pub mod plan;
pub mod resolve;
pub mod toolchain;
