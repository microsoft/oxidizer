// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise benchmarks for Internity's hot paths.

#![allow(missing_docs, reason = "benchmark target")]

// Gungraun requires Valgrind, which is Linux-only. Keep the target buildable on
// other platforms so workspace-wide all-target checks succeed.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = ops);
