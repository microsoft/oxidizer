// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "Benchmarks don't require documentation and should fail fast on errors"
)]

use arty::Spawner;
use criterion::{Criterion, criterion_group, criterion_main};

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawner");

    // Tokio benchmarks
    let rt = tokio::runtime::Runtime::new().unwrap();
    let tokio_spawner = Spawner::tokio();

    group.bench_function("tokio_direct", |b| {
        b.iter(|| rt.block_on(async { tokio::spawn(async { 42 }).await.unwrap() }));
    });

    group.bench_function("tokio_via_spawner", |b| {
        b.iter(|| rt.block_on(async { tokio_spawner.spawn(async { 42 }).await }));
    });

    // async-std benchmarks
    let async_std_spawner = Spawner::custom(|fut| {
        async_std::task::spawn(fut);
    });

    group.bench_function("async_std_direct", |b| {
        b.iter(|| async_std::task::block_on(async { async_std::task::spawn(async { 42 }).await }));
    });

    group.bench_function("async_std_via_spawner", |b| {
        b.iter(|| async_std::task::block_on(async { async_std_spawner.spawn(async { 42 }).await }));
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);
