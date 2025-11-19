// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::System;
use std::hint::black_box;
use std::iter;
use std::num::NonZero;

use alloc_tracker::{Allocator, Session};
use byte_sequences::{BlockSize, BytesView, FixedBlockTestMemory};
use bytes::Buf;
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

    let test_data_as_seq = BytesView::copied_from_slice(TEST_DATA, &memory);

    let max_inline = iter::repeat_n(test_data_as_seq.clone(), MAX_INLINE_SPANS).collect::<Vec<_>>();

    let many = iter::repeat_n(test_data_as_seq.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_seq = BytesView::from_sequences(many.iter().cloned());
    let many_as_bytes = many_as_seq.clone().into_bytes();

    let ten = iter::repeat_n(test_data_as_seq.clone(), 10).collect::<Vec<_>>();
    let ten_as_seq = BytesView::from_sequences(ten.iter().cloned());

    let mut group = c.benchmark_group("BytesView");

    let allocs_op = allocs.operation("new");
    group.bench_function("new", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            BytesView::new()
        });
    });

    group.bench_function("len", |b| {
        b.iter(|| test_data_as_seq.len());
    });

    group.bench_function("len_many", |b| {
        b.iter(|| many_as_seq.len());
    });

    group.bench_function("is_empty", |b| {
        b.iter(|| test_data_as_seq.is_empty());
    });

    let allocs_op = allocs.operation("extend_lifetime");
    group.bench_function("extend_lifetime", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            test_data_as_seq.extend_lifetime()
        });
    });

    let allocs_op = allocs.operation("extend_lifetime_many");
    group.bench_function("extend_lifetime_many", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            many_as_seq.extend_lifetime()
        });
    });

    let allocs_op = allocs.operation("slice_near");
    group.bench_function("slice_near", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            test_data_as_seq.slice(black_box(0..10))
        });
    });

    let allocs_op = allocs.operation("slice_far");
    group.bench_function("slice_far", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            test_data_as_seq.slice(black_box(12300..12310))
        });
    });

    let allocs_op = allocs.operation("slice_very_far");
    group.bench_function("slice_very_far", |b| {
        // There are 10 spans in this sequence, with our slice being from the last one.
        b.iter(|| {
            let _span = allocs_op.measure_thread();
            ten_as_seq.slice(black_box(123_000..123_010))
        });
    });

    let allocs_op = allocs.operation("consume_all_chunks");
    group.bench_function("consume_all_chunks", |b| {
        b.iter_batched_ref(
            || test_data_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                seq.consume_all_chunks(|chunk| {
                    _ = black_box(chunk);
                });
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("chunks_as_slices_vectored", |b| {
        b.iter(|| {
            // Will only fill 1 of 4 slots, since the test data is just one chunk.
            let mut slices: Vec<&[u8]> = vec![&[]; 4];
            test_data_as_seq.chunks_as_slices_vectored(&mut slices);

            _ = black_box(slices);
        });
    });

    group.bench_function("advance_one_byte", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                seq.advance(1);
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("advance_one_span", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                seq.advance(TEST_SPAN_SIZE.get() as usize);
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("advance_all_spans", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                seq.advance(TEST_SPAN_SIZE.get() as usize * 10);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("to_bytes_single_chunk");
    group.bench_function("to_bytes_single_chunk", |b| {
        let seq = BytesView::from(test_data_as_seq.clone().into_bytes());

        b.iter(|| {
            let _span = allocs_op.measure_process();
            let _bytes = seq.clone().into_bytes();
        });
    });

    group.finish();

    let mut group = c.benchmark_group("BytesView_slow");

    group.bench_function("eq_self", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |other| {
                assert!(black_box(many_as_seq == *other));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("eq_slice", |b| {
        b.iter_batched_ref(
            || many_as_bytes.chunk(),
            |other| {
                assert!(black_box(many_as_seq == *other));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("to_bytes_many_chunks");
    group.bench_function("to_bytes_many_chunks", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_process();
            let _bytes = many_as_seq.clone().into_bytes();
        });
    });

    let allocs_op = allocs.operation("from_many");
    group.bench_function("from_many", |b| {
        b.iter_batched(
            || many.iter().cloned(),
            |many_clones| {
                let _span = allocs_op.measure_thread();
                BytesView::from_sequences(black_box(many_clones))
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("clone_many");
    group.bench_function("clone_many", |b| {
        b.iter(|| {
            let _span = allocs_op.measure_process();
            let _sequence = many_as_seq.clone();
        });
    });

    let allocs_op = allocs.operation("from_max_inline");
    group.bench_function("from_max_inline", |b| {
        b.iter_batched(
            || max_inline.iter().cloned(),
            |max_inline_clones| {
                let _span = allocs_op.measure_thread();
                BytesView::from_sequences(black_box(max_inline_clones))
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    allocs.print_to_stdout();
}
