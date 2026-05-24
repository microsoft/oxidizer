// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]

//! Criterion wall-clock dispatch benchmarks for `sync_thunk`.
//!
//! Mirrors `benches/gungraun_dispatch.rs` 1:1: each `dispatch/<variant>` here
//! corresponds to a gungraun function `dispatch_<variant>`.
//!
//! Compares the per-call dispatch cost of `#[thunk]` against
//! `tokio::task::spawn_blocking` on identical trivial workloads.
//!
//! Run with: `cargo bench --bench criterion_dispatch`

#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

mod common;

use common::{Fixture, N, run_spawn_blocking_arg_u64, run_spawn_blocking_void, run_thunk_arg_u64, run_thunk_void};

fn bench_dispatch(c: &mut Criterion) {
    let fixture = Fixture::new();

    let mut g = c.benchmark_group("dispatch");
    // Each iteration performs N awaits; report per-call throughput.
    g.throughput(Throughput::Elements(N as u64));

    g.bench_function("thunk_void", |b| {
        b.iter(|| run_thunk_void(&fixture));
    });

    g.bench_function("thunk_arg_u64", |b| {
        b.iter(|| run_thunk_arg_u64(&fixture));
    });

    g.bench_function("spawn_blocking_void", |b| {
        b.iter(|| run_spawn_blocking_void(&fixture));
    });

    g.bench_function("spawn_blocking_arg_u64", |b| {
        b.iter(|| run_spawn_blocking_arg_u64(&fixture));
    });

    g.finish();
}

criterion_group!(benches, bench_dispatch);
criterion_main!(benches);
