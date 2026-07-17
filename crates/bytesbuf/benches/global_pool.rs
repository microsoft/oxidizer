// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;

use alloc_tracker::{Allocator, Session};
use benchmarking::{time_sample, time_sample_with_inputs};
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use criterion::{Criterion, criterion_group, criterion_main};
use testing_aids::repeating_incrementing_bytes;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const ONE_MB: usize = 1024 * 1024;
const TINY: usize = 128;

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let warm_memory = GlobalPool::new();

    // Allocate some memory to pre-warm the pool.
    drop(warm_memory.reserve(10 * ONE_MB));
    drop(warm_memory.reserve(TINY));

    let mut group = c.benchmark_group("GlobalPool");

    let allocs_op = allocs.operation("alloc_tiny");
    group.bench_function("alloc_tiny", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample(|| warm_memory.reserve(TINY))(iters)
        });
    });

    let allocs_op = allocs.operation("alloc_1mb");
    group.bench_function("alloc_1mb", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample(|| warm_memory.reserve(ONE_MB))(iters)
        });
    });

    let allocs_op = allocs.operation("fill_tiny");
    group.bench_function("fill_tiny", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample(|| {
                let mut buf = warm_memory.reserve(TINY);
                buf.put_byte_repeated(66, TINY);
            })(iters)
        });
    });

    let allocs_op = allocs.operation("fill_1mb");
    group.bench_function("fill_1mb", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample(|| {
                let mut buf = warm_memory.reserve(ONE_MB);
                buf.put_byte_repeated(66, ONE_MB);
            })(iters)
        });
    });

    let allocs_op = allocs.operation("fill_tiny_cold");
    group.bench_function("fill_tiny_cold", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample_with_inputs(GlobalPool::new, |memory| {
                let mut buf = memory.reserve(TINY);
                buf.put_byte_repeated(66, TINY);
            })(iters)
        });
    });

    let allocs_op = allocs.operation("fill_1mb_cold");
    group.bench_function("fill_1mb_cold", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample_with_inputs(GlobalPool::new, |memory| {
                let mut buf = memory.reserve(ONE_MB);
                buf.put_byte_repeated(66, ONE_MB);
            })(iters)
        });
    });

    let test_data = repeating_incrementing_bytes().take(ONE_MB).collect::<Vec<u8>>();

    let allocs_op = allocs.operation("copied_from_slice");
    group.bench_function("copied_from_slice", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample(|| BytesView::copied_from_slice(&test_data, &warm_memory))(iters)
        });
    });

    let allocs_op = allocs.operation("copied_from_slice_cold");
    group.bench_function("copied_from_slice_cold", |b| {
        b.iter_custom(|iters| {
            let _span = allocs_op.measure_thread().iterations(iters);
            time_sample_with_inputs(GlobalPool::new, |memory| BytesView::copied_from_slice(&test_data, &memory))(iters)
        });
    });

    group.finish();
}
