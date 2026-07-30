// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise benchmarks for Internity's hot paths.
//!
//! Paired with `internity_compare.rs` which covers the same single-threaded
//! operations under wall-clock measurement.

#![allow(missing_docs, reason = "benchmark target")]

// Gungraun requires Valgrind, which is Linux-only. Keep the target buildable on
// other platforms so workspace-wide all-target checks succeed.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
#[path = "counts/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{insert, lookup, reuse};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = insert, reuse, lookup);
