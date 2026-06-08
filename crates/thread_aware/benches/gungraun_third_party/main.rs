// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Instruction-precise benchmarks for `ThreadAware` impls on 3rd-party types.
//!
//! Mirrors `benches/criterion_third_party.rs` 1:1: each gungraun function
//! `<group>_<variant>` corresponds to a criterion benchmark `<group>/<variant>`.
//!
//! Run with: `cargo bench -p thread_aware --bench gungraun_third_party \
//!     --features "bytes_v1 http_v1 jiff_v0_2 uuid_v1"` on a Linux host
//! with Valgrind installed.

#![allow(missing_docs, reason = "benchmark code")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]
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
    library_benchmark_groups = third_party_group
);
