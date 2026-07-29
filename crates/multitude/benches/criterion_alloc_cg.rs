// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind allocation benchmarks for multitude.
//!
//! Paired with `criterion_alloc.rs`: each function named
//! `<group>_<variant>` corresponds to
//! `criterion_alloc/<group>/<variant>`.
//!
//! Run with `cargo bench --bench criterion_alloc_cg` on a Linux host with
//! Valgrind.

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
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

// Gungraun requires Valgrind, which is Linux-only. On other platforms this
// bench target compiles to a no-op so `cargo build --all-targets` still works.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
#[path = "criterion_alloc_cg/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = gungraun::LibraryBenchmarkConfig::default()
        .tool(gungraun::Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups =
        arena_lifecycle,
        alloc_u64,
        alloc_str,
        alloc_slice,
        string_builder,
        vec_builder,
        allocator_grow
);
