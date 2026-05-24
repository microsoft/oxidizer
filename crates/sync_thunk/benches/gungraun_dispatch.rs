// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]

//! Instruction-precise dispatch benchmarks for `sync_thunk`.
//!
//! Mirrors `benches/criterion_dispatch.rs` 1:1: each gungraun function
//! `dispatch_<variant>` corresponds to a criterion benchmark
//! `dispatch/<variant>`.
//!
//! Compares the per-call dispatch cost of `#[thunk]` against
//! `tokio::task::spawn_blocking` on identical trivial workloads.
//!
//! Run with `cargo bench --bench gungraun_dispatch` on a Linux host with
//! Valgrind installed.
//!
//! # Interpretation
//!
//! Valgrind serialises threads, so wall-clock numbers under gungraun are
//! meaningless and cross-core cache-coherence costs are invisible. What
//! gungraun *does* measure precisely is the user-space instruction count of
//! the dispatch path itself — exactly the metric `#[thunk]` exists to
//! optimise relative to `spawn_blocking`. Pair with `criterion_dispatch` for
//! wall-clock context.

#![expect(missing_docs, reason = "Benchmark")]
#![allow(unused_results, reason = "black_box of bench input is intentional")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun bench inputs are passed by value by the framework"
)]

use gungraun::{Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};

mod common;

use common::{Fixture, run_spawn_blocking_arg_u64, run_spawn_blocking_void, run_thunk_arg_u64, run_thunk_void};

#[library_benchmark]
#[bench::run(Fixture::new())]
fn dispatch_thunk_void(fixture: Fixture) -> Fixture {
    run_thunk_void(&fixture);
    fixture
}

#[library_benchmark]
#[bench::run(Fixture::new())]
fn dispatch_thunk_arg_u64(fixture: Fixture) -> Fixture {
    run_thunk_arg_u64(&fixture);
    fixture
}

#[library_benchmark]
#[bench::run(Fixture::new())]
fn dispatch_spawn_blocking_void(fixture: Fixture) -> Fixture {
    run_spawn_blocking_void(&fixture);
    fixture
}

#[library_benchmark]
#[bench::run(Fixture::new())]
fn dispatch_spawn_blocking_arg_u64(fixture: Fixture) -> Fixture {
    run_spawn_blocking_arg_u64(&fixture);
    fixture
}

library_benchmark_group!(
    name = dispatch_group;
    benchmarks =
        dispatch_thunk_void,
        dispatch_thunk_arg_u64,
        dispatch_spawn_blocking_void,
        dispatch_spawn_blocking_arg_u64
);

main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = dispatch_group
);
