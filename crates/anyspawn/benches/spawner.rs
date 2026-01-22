// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    reason = "Benchmarks don't require documentation and should fail fast on errors"
)]

use anyspawn::Spawner;
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

    // smol benchmarks
    let smol_spawner = Spawner::custom(|fut| {
        smol::spawn(fut).detach();
    });

    group.bench_function("smol_direct", |b| {
        b.iter(|| smol::block_on(async { smol::spawn(async { 42 }).await }));
    });

    group.bench_function("smol_via_spawner", |b| {
        b.iter(|| smol::block_on(async { smol_spawner.spawn(async { 42 }).await }));
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);
