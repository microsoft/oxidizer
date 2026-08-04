// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind `Rc<[Rc<[u8]>]>` build benchmarks for multitude.
//!
//! Paired with `criterion_rc_array.rs`: each function named
//! `rc_array_<variant>` corresponds to
//! `criterion_rc_array/rc_array/<variant>`.
//!
//! Run with `cargo bench --bench criterion_rc_array_cg` on a Linux host with
//! Valgrind.

#![allow(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]
#![allow(clippy::type_complexity, reason = "benchmark state tuples are inherently complex")]
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
#[path = "criterion_rc_array_cg/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::*;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = gungraun::LibraryBenchmarkConfig::default()
        .tool(gungraun::Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = rc_array
);
