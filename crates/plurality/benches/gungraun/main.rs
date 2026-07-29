// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind (instruction-count) benchmarks for the allocation functions and
//! the owning fat-pointer comparison.
//!
//! gungraun needs Valgrind (Linux-only); on other targets this bench compiles
//! to a no-op so `cargo build --all-targets` still works. The benchmark bodies
//! live in `linux.rs`.

#![cfg_attr(
    target_os = "linux",
    allow(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "triggered by the gungraun main! macro expansion"
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod ops;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = gungraun::LibraryBenchmarkConfig::default()
        .tool(gungraun::Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = alloc, clone, dyn_box
);
