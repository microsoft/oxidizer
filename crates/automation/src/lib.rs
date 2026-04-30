// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An unpublished crate for shared code used for writing Rust scripts

#![allow(clippy::missing_errors_doc, reason = "this is an internal crate for scripts")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

mod cargo;
mod cargo_metadata;
mod process;

pub use cargo::{INTERNAL_CRATES, run_cargo};
pub use cargo_metadata::{PackageMetadata, Target, list_packages};
pub use process::{Outcome, RunResult, run_with_timeout};
