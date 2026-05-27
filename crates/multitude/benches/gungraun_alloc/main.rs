// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise allocation benchmarks for multitude.
//!
//! Mirrors `benches/criterion_alloc.rs` 1:1: each gungraun function
//! `<group>_<variant>` corresponds to a criterion benchmark
//! `<group>/<variant>`.
//!
//! Run with `cargo bench --bench gungraun_alloc` on a Linux host with Valgrind.

#![allow(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]
#![allow(clippy::ref_as_ptr, reason = "trivial pointer cast in bench plumbing")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

// Gungraun requires Valgrind, which is Linux-only. On other platforms this
// bench target compiles to a no-op so `cargo build --all-targets` still works.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = gungraun::LibraryBenchmarkConfig::default()
        .tool(gungraun::Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = alloc_group
);
