// Copyright (c) Microsoft Corporation.

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use futures::executor::block_on;
use layered::{DynamicServiceExt, Execute, Intercept, Service, ServiceBuilder};
use oxidizer_benchmarking::BenchmarkGroupExt;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

pub fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("typed-vs-dynamic");
    let session = Session::new();

    let service = Execute::new(|v| async move { v });
    group.bench_with_memory(|| _ = block_on(service.execute(10)), "typed", &session);

    let service = Execute::new(|v| async move { v }).into_dynamic();
    group.bench_with_memory(|| _ = block_on(service.execute(10)), "dynamic", &session);

    let service = (Intercept::layer(), Execute::new(|v| async move { v })).build();
    group.bench_with_memory(|| _ = block_on(service.execute(10)), "wrapped_typed", &session);

    let service = (Intercept::layer(), Execute::new(|v| async move { v })).build().into_dynamic();
    group.bench_with_memory(|| _ = block_on(service.execute(10)), "wrapped_dynamic", &session);

    session.print_to_stdout();
}

criterion_group!(benches, entry);
criterion_main!(benches);
