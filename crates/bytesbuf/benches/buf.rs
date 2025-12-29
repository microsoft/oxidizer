// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;
use std::hint::black_box;
use std::iter;
use std::num::NonZero;

use alloc_tracker::{Allocator, Session};
use bytesbuf::{BlockSize, BytesBuf, BytesView, FixedBlockTestMemory, TransparentTestMemory};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use new_zealand::nz;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

// The test data is "HTTP request sized". Ultimately, we expect most operations to be zero-copy,
// so the size of the test data should not matter much, unless we try reading it all at once.
const TEST_SPAN_SIZE: NonZero<BlockSize> = nz!(12345);
const TEST_DATA: &[u8] = &[88_u8; TEST_SPAN_SIZE.get() as usize];

const MAX_INLINE_SPANS: usize = bytesbuf::MAX_INLINE_SPANS;
// This should be more than MAX_INLINE_SPANS.
const MANY_SPANS: usize = 32;

#[expect(clippy::too_many_lines, reason = "Is fine - lots of benchmarks to do!")]
fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let memory = TransparentTestMemory::new();

    let test_data_as_view = BytesView::copied_from_slice(TEST_DATA, &memory);

    let max_inline = iter::repeat_n(test_data_as_view.clone(), MAX_INLINE_SPANS).collect::<Vec<_>>();
    let max_inline_as_view = BytesView::from_views(max_inline.iter().cloned());

    let many = iter::repeat_n(test_data_as_view.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_view = BytesView::from_views(many.iter().cloned());

    let mut group = c.benchmark_group("BytesBuf");

    let new_allocs = allocs.operation("new");
    group.bench_function("new", |b| {
        b.iter(|| {
            let _span = new_allocs.measure_thread();
            BytesBuf::new()
        });
    });

    group.bench_function("len_empty", |b| {
        b.iter_batched_ref(BytesBuf::new, |buf| buf.len(), BatchSize::SmallInput);
    });

    group.bench_function("len_many", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(many_as_view.clone());
                buf
            },
            |buf| buf.len(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("is_empty_empty", |b| {
        b.iter_batched_ref(BytesBuf::new, |buf| buf.is_empty(), BatchSize::SmallInput);
    });

    group.bench_function("is_empty_many", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(many_as_view.clone());
                buf
            },
            |buf| buf.is_empty(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("capacity_empty", |b| {
        b.iter_batched_ref(BytesBuf::new, |buf| buf.capacity(), BatchSize::SmallInput);
    });

    group.bench_function("capacity_many", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(many_as_view.clone());
                buf
            },
            |buf| buf.capacity(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("reserve", |b| {
        b.iter_batched_ref(BytesBuf::new, |buf| buf.reserve(black_box(1), &memory), BatchSize::SmallInput);
    });

    let allocs_op = allocs.operation("put_view_clean");
    group.bench_function("put_view_clean", |b| {
        b.iter_batched_ref(
            BytesBuf::new,
            |buf| {
                let _span = allocs_op.measure_thread();
                buf.put_bytes(test_data_as_view.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_view_dirty");
    group.bench_function("put_view_dirty", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.reserve(TEST_SPAN_SIZE.get() as usize, &memory);
                buf.put_byte(123);
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                buf.put_bytes(test_data_as_view.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_one_span");
    group.bench_function("consume_one_span", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(many_as_view.clone());
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                buf.consume(TEST_SPAN_SIZE.get() as usize)
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_max_inline_spans");
    group.bench_function("consume_max_inline_spans", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(max_inline_as_view.clone());
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                buf.consume(TEST_SPAN_SIZE.get() as usize);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_many_spans");
    group.bench_function("consume_many_spans", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(many_as_view.clone());
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                buf.consume_all()
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("extend_lifetime");
    group.bench_function("extend_lifetime", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(test_data_as_view.clone());
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                buf.extend_lifetime()
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("vectored_write_one_span");
    group.bench_function("vectored_write_one_span", |b| {
        const BLOCK_SIZE: NonZero<BlockSize> = nz!(10);
        let memory = FixedBlockTestMemory::new(BLOCK_SIZE);

        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.reserve(BLOCK_SIZE.get() as usize, &memory);
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                let write = buf.begin_vectored_write(None);

                // SAFETY: Yes, I promise I wrote this many bytes.
                // This is a lie but we do not touch the bytes, so should be a harmless lie.
                unsafe {
                    write.commit(BLOCK_SIZE.get() as usize);
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("vectored_write_max_inline_spans");
    group.bench_function("vectored_write_max_inline_spans", |b| {
        const BLOCK_SIZE: NonZero<BlockSize> = nz!(10);
        let memory = FixedBlockTestMemory::new(BLOCK_SIZE);

        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.reserve(BLOCK_SIZE.get() as usize * MAX_INLINE_SPANS, &memory);
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                let write = buf.begin_vectored_write(None);

                // SAFETY: Yes, I promise I wrote this many bytes.
                // This is a lie but we do not touch the bytes, so should be a harmless lie.
                unsafe {
                    write.commit(BLOCK_SIZE.get() as usize * MAX_INLINE_SPANS);
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("vectored_write_many_spans");
    group.bench_function("vectored_write_many_spans", |b| {
        const BLOCK_SIZE: NonZero<BlockSize> = nz!(10);
        let memory = FixedBlockTestMemory::new(BLOCK_SIZE);

        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.reserve(BLOCK_SIZE.get() as usize * MANY_SPANS, &memory);
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                let write = buf.begin_vectored_write(None);

                // SAFETY: Yes, I promise I wrote this many bytes.
                // This is a lie but we do not touch the bytes, so should be a harmless lie.
                unsafe {
                    write.commit(BLOCK_SIZE.get() as usize * MANY_SPANS);
                }
            },
            BatchSize::SmallInput,
        );
    });

    // Current implementation limits advance_mut() to one span - can only advance more via
    // the vectored write API.
    group.bench_function("advance_mut_one_span", |b| {
        const BLOCK_SIZE: NonZero<BlockSize> = nz!(10);
        let memory = FixedBlockTestMemory::new(BLOCK_SIZE);

        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.reserve(BLOCK_SIZE.get() as usize, &memory);
                buf
            },
            |buf| {
                // SAFETY: Yes, I promise I wrote this many bytes.
                // This is a lie but we do not touch the bytes, so should be a harmless lie.
                unsafe {
                    buf.advance(BLOCK_SIZE.get() as usize);
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("peek_frozen_all");
    group.bench_function("peek_frozen_all", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.put_bytes(many_as_view.clone());
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                let mut peeked = buf.peek();

                // We just seek to the end, that is all.
                while !peeked.is_empty() {
                    peeked.advance(peeked.first_slice().len());
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("peek_unfrozen_all");
    group.bench_function("peek_unfrozen_all", |b| {
        b.iter_batched_ref(
            || {
                let mut buf = BytesBuf::new();
                buf.reserve(TEST_SPAN_SIZE.get() as usize, &memory);
                buf.put_byte(123);
                buf
            },
            |buf| {
                let _span = allocs_op.measure_thread();
                let mut peeked = buf.peek();

                // We just seek to the end, that is all.
                while !peeked.is_empty() {
                    peeked.advance(peeked.first_slice().len());
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    allocs.print_to_stdout();
}
