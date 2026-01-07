// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;

use alloc_tracker::{Allocator, Session};
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use testing_aids::repeating_incrementing_bytes;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const ONE_MB: usize = 1024 * 1024;

fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let warm_memory = GlobalPool::new();

    // Allocate some memory to pre-warm the pool.
    drop(warm_memory.reserve(10 * ONE_MB));

    let mut group = c.benchmark_group("GlobalPool");

    let allocs_op = allocs.operation("fill_1mb");
    group.bench_function("fill_1mb", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            let mut buf = warm_memory.reserve(ONE_MB);
            buf.put_byte_repeated(66, ONE_MB);
        });
    });

    let allocs_op = allocs.operation("fill_1mb_cold");
    group.bench_function("fill_1mb_cold", |b| {
        b.iter_batched(
            GlobalPool::new,
            |memory| {
                let _span = allocs_op.measure_thread();
                let mut buf = memory.reserve(ONE_MB);
                buf.put_byte_repeated(66, ONE_MB);
            },
            BatchSize::LargeInput,
        );
    });

    let test_data = repeating_incrementing_bytes().take(ONE_MB).collect::<Vec<u8>>();

    let allocs_op = allocs.operation("copied_from_slice");
    group.bench_function("copied_from_slice", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            BytesView::copied_from_slice(&test_data, &warm_memory)
        });
    });

    let allocs_op = allocs.operation("copied_from_slice_cold");
    group.bench_function("copied_from_slice_cold", |b| {
        b.iter_batched(
            GlobalPool::new,
            |memory| {
                let _span = allocs_op.measure_thread();
                BytesView::copied_from_slice(&test_data, &memory)
            },
            BatchSize::LargeInput,
        );
    });

    group.finish();

    allocs.print_to_stdout();
}
