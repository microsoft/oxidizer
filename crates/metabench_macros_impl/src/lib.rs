// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros_impl/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros_impl/favicon.ico"
)]

//! Implementation of the procedural macros for the `metabench` crate.
//!
//! **Do not depend on this crate directly.** Use the re-exports from
//! `metabench` instead.
//!
//! The public `metabench` facade is the supported macro interface and provides
//! invocation examples.
//!
//! ```rust
//! use metabench_macros_impl::benchmark;
//! ```

mod benchmark;
mod shared;
mod target;

pub use benchmark::benchmark;
pub use target::main;
