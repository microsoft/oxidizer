// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise drop benchmarks for multitude.
//!
//! Mirrors `benches/criterion_drop.rs` 1:1: each gungraun function
//! `drop_<variant>` corresponds to a criterion benchmark `drop/<variant>`.
//! Each setup pre-fills an arena with N handles; the bench body drops them
//! (handle vec + arena), measuring per-handle smart-pointer drop plus chunk
//! teardown at arena drop.

#![allow(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
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
    library_benchmark_groups = drop_group
);
