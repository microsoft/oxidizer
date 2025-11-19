// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::System;
use std::hint::black_box;
use std::iter;
use std::num::NonZero;

use alloc_tracker::{Allocator, Session};
use byte_sequences::{BlockSize, BytesView, BytesBuf, FixedBlockTestMemory};
use bytes::{Buf, BufMut};
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

const MAX_INLINE_SPANS: usize = byte_sequences::MAX_INLINE_SPANS;
// This should be more than MAX_INLINE_SPANS.
const MANY_SPANS: usize = 32;

#[allow(clippy::too_many_lines, reason = "Is fine - lots of benchmarks to do!")]
fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let memory = FixedBlockTestMemory::new(TEST_SPAN_SIZE);

    let test_data_as_seq = BytesView::copy_from_slice(TEST_DATA, &memory);

    let max_inline = iter::repeat_n(test_data_as_seq.clone(), MAX_INLINE_SPANS).collect::<Vec<_>>();
    let max_inline_as_seq = BytesView::from_sequences(max_inline.iter().cloned());

    let many = iter::repeat_n(test_data_as_seq.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_seq = BytesView::from_sequences(many.iter().cloned());

    let mut group = c.benchmark_group("BytesBuf");

    let new_allocs = allocs.operation("new");
    group.bench_function("new", |b| {
        b.iter(|| {
            let _span = new_allocs.measure_thread();
            BytesBuf::new()
        });
    });

    group.bench_function("len_empty", |b| {
        b.iter_batched_ref(BytesBuf::new, |sb| sb.len(), BatchSize::SmallInput);
    });

    group.bench_function("len_many", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(many_as_seq.clone());
                sb
            },
            |sb| sb.len(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("is_empty_empty", |b| {
        b.iter_batched_ref(BytesBuf::new, |sb| sb.is_empty(), BatchSize::SmallInput);
    });

    group.bench_function("is_empty_many", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(many_as_seq.clone());
                sb
            },
            |sb| sb.is_empty(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("capacity_empty", |b| {
        b.iter_batched_ref(BytesBuf::new, |sb| sb.capacity(), BatchSize::SmallInput);
    });

    group.bench_function("capacity_many", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(many_as_seq.clone());
                sb
            },
            |sb| sb.capacity(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("reserve", |b| {
        b.iter_batched_ref(
            BytesBuf::new,
            |sb| sb.reserve(black_box(1), &memory),
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("append_clean");
    group.bench_function("append_clean", |b| {
        b.iter_batched_ref(
            BytesBuf::new,
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.append(test_data_as_seq.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("append_dirty");
    group.bench_function("append_dirty", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(TEST_SPAN_SIZE.get() as usize, &memory);
                sb.put_u8(123);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.append(test_data_as_seq.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_one_span");
    group.bench_function("consume_one_span", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(many_as_seq.clone());
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.consume(TEST_SPAN_SIZE.get() as usize)
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_max_inline_spans");
    group.bench_function("consume_max_inline_spans", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(max_inline_as_seq.clone());
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.consume(TEST_SPAN_SIZE.get() as usize);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_many_spans");
    group.bench_function("consume_many_spans", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(many_as_seq.clone());
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.consume_all()
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("extend_lifetime");
    group.bench_function("extend_lifetime", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(test_data_as_seq.clone());
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.extend_lifetime()
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
                let mut sb = BytesBuf::new();
                sb.reserve(BLOCK_SIZE.get() as usize, &memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let write = sb.begin_vectored_write(None);

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
                let mut sb = BytesBuf::new();
                sb.reserve(BLOCK_SIZE.get() as usize * MAX_INLINE_SPANS, &memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let write = sb.begin_vectored_write(None);

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
                let mut sb = BytesBuf::new();
                sb.reserve(BLOCK_SIZE.get() as usize * MANY_SPANS, &memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let write = sb.begin_vectored_write(None);

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
                let mut sb = BytesBuf::new();
                sb.reserve(BLOCK_SIZE.get() as usize, &memory);
                sb
            },
            |sb| {
                // SAFETY: Yes, I promise I wrote this many bytes.
                // This is a lie but we do not touch the bytes, so should be a harmless lie.
                unsafe {
                    sb.advance_mut(BLOCK_SIZE.get() as usize);
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("inspect_frozen_all");
    group.bench_function("inspect_frozen_all", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.append(many_as_seq.clone());
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let mut inspector = sb.inspect();

                // We just seek to the end, that is all.
                while inspector.has_remaining() {
                    inspector.advance(inspector.chunk().len());
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("inspect_unfrozen_all");
    group.bench_function("inspect_unfrozen_all", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(TEST_SPAN_SIZE.get() as usize, &memory);
                sb.put_u8(123);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let mut inspector = sb.inspect();

                // We just seek to the end, that is all.
                while inspector.has_remaining() {
                    inspector.advance(inspector.chunk().len());
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    allocs.print_to_stdout();
}
