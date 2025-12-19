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
const PUT_BYTES_LEN: usize = 512;

#[expect(clippy::too_many_lines, reason = "Is fine - lots of benchmarks to do!")]
fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let memory = FixedBlockTestMemory::new(TEST_SPAN_SIZE);
    let transparent_memory = TransparentTestMemory::new();

    let test_data_as_seq = BytesView::copied_from_slice(TEST_DATA, &memory);

    let max_inline = iter::repeat_n(test_data_as_seq.clone(), MAX_INLINE_SPANS).collect::<Vec<_>>();
    let max_inline_as_seq = BytesView::from_views(max_inline.iter().cloned());

    let many = iter::repeat_n(test_data_as_seq.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_seq = BytesView::from_views(many.iter().cloned());

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
                sb.put_bytes(many_as_seq.clone());
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
                sb.put_bytes(many_as_seq.clone());
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
                sb.put_bytes(many_as_seq.clone());
                sb
            },
            |sb| sb.capacity(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("reserve", |b| {
        b.iter_batched_ref(BytesBuf::new, |sb| sb.reserve(black_box(1), &memory), BatchSize::SmallInput);
    });

    let allocs_op = allocs.operation("put_f64_be");
    group.bench_function("put_f64_be", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<f64>(black_box(1234.5678));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u64_be");
    group.bench_function("put_u64_be", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<u64>(black_box(0x1234_5678_9ABC_DEF0));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_f64_le");
    group.bench_function("put_f64_le", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<f64>(black_box(8765.4321));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u64_le");
    group.bench_function("put_u64_le", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<u64>(black_box(0x0FED_CBA9_8765_4321));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u8");
    group.bench_function("put_u8", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(1, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<u8>(black_box(0xAB));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_bytes");
    group.bench_function("put_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(PUT_BYTES_LEN, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_byte_repeated(0xCD, PUT_BYTES_LEN);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_buf");
    group.bench_function("put", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(test_data_as_seq.len(), &memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_bytes(test_data_as_seq.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_view_clean");
    group.bench_function("put_view_clean", |b| {
        b.iter_batched_ref(
            BytesBuf::new,
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_bytes(test_data_as_seq.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_view_dirty");
    group.bench_function("put_view_dirty", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(TEST_SPAN_SIZE.get() as usize, &memory);
                sb.put_byte(123);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_bytes(test_data_as_seq.clone());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("consume_one_span");
    group.bench_function("consume_one_span", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.put_bytes(many_as_seq.clone());
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
                sb.put_bytes(max_inline_as_seq.clone());
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
                sb.put_bytes(many_as_seq.clone());
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
                sb.put_bytes(test_data_as_seq.clone());
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
                    sb.advance(BLOCK_SIZE.get() as usize);
                }
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("peek_frozen_all");
    group.bench_function("peek_frozen_all", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.put_bytes(many_as_seq.clone());
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let mut peeked = sb.peek();

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
                let mut sb = BytesBuf::new();
                sb.reserve(TEST_SPAN_SIZE.get() as usize, &memory);
                sb.put_byte(123);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let mut peeked = sb.peek();

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
