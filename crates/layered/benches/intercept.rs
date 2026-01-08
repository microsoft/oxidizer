// Copyright (c) Microsoft Corporation.

#![allow(missing_docs, reason = "Benchmarks don't require documentation")]

use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{Execute, Intercept, Service, Stack};

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("intercept");

    let service = Execute::new(|v| async move { v });
    group.bench_function("plain", |b| b.iter(|| block_on(service.execute(0))));

    let service = (Intercept::layer(), Execute::new(|v| async move { v })).build();
    group.bench_function("intercept-empty", |b| {
        b.iter(|| block_on(service.execute(0)));
    });

    let service = (Intercept::layer().on_input(|_v| {}), Execute::new(|v| async move { v })).build();
    group.bench_function("on-input", |b| b.iter(|| block_on(service.execute(0))));

    let service = (Intercept::layer().modify_input(|v| v + 1), Execute::new(|v| async move { v })).build();
    group.bench_function("modify-input", |b| b.iter(|| block_on(service.execute(0))));

    let service = (Intercept::layer().on_output(|_v| {}), Execute::new(|v| async move { v })).build();
    group.bench_function("on-output", |b| b.iter(|| block_on(service.execute(0))));

    let service = (Intercept::layer().modify_output(|v| v + 1), Execute::new(|v| async move { v })).build();
    group.bench_function("modify-output", |b| b.iter(|| block_on(service.execute(0))));

    let service = (
        Intercept::layer()
            .on_input(|_v| {})
            .on_input(|_v| {})
            .modify_input(|v| v + 1)
            .modify_input(|v| v + 1)
            .on_output(|_v| {})
            .on_output(|_v| {})
            .modify_output(|v| v + 1)
            .modify_output(|v| v + 1),
        Execute::new(|v| async move { v }),
    )
        .build();
    group.bench_function("complex", |b| b.iter(|| block_on(service.execute(0))));
}

criterion_group!(benches, entry);
criterion_main!(benches);
