// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;
use std::hint::black_box;
use std::num::NonZero;

use alloc_tracker::{Allocator, Session};
use bytes::{Buf, BufMut};
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

const MANY_SPANS: usize = 32;
const COPY_TO_SLICE_LEN: usize = 256;

#[expect(clippy::too_many_lines, reason = "Is fine - lots of benchmarks to do!")]
fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let memory = FixedBlockTestMemory::new(TEST_SPAN_SIZE);
    let transparent_memory = TransparentTestMemory::new();

    // Prepare test data - a multi-span view for reading operations
    let test_data_as_seq = BytesView::copied_from_slice(TEST_DATA, &memory);
    let many = std::iter::repeat_n(test_data_as_seq.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_seq = BytesView::from_views(many.iter().cloned());

    // ============================================================================
    // GET operations (slower, non-numeric)
    // ============================================================================

    let mut group = c.benchmark_group("get");

    let allocs_op = allocs.operation("get_byte_bytesbuf");
    group.bench_function("get_byte_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_byte());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u8_bytes");
    group.bench_function("get_u8_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u8());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("copy_to_slice_bytesbuf");
    group.bench_function("copy_to_slice_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                let mut target = [0u8; COPY_TO_SLICE_LEN];
                seq.copy_to_slice(&mut target);
                black_box(target);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("copy_to_slice_bytes");
    group.bench_function("copy_to_slice_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                let mut target = [0u8; COPY_TO_SLICE_LEN];
                seq.copy_to_slice(&mut target);
                black_box(target);
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    // ============================================================================
    // PUT operations (slower, non-numeric)
    // ============================================================================

    let mut group = c.benchmark_group("put");

    let allocs_op = allocs.operation("put_slice_bytesbuf");
    group.bench_function("put_slice_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(COPY_TO_SLICE_LEN, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let data = [0xCD_u8; COPY_TO_SLICE_LEN];
                sb.put_slice(data);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_slice_bytes");
    group.bench_function("put_slice_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(COPY_TO_SLICE_LEN, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                let data = [0xCD_u8; COPY_TO_SLICE_LEN];
                sb.put_slice(&data[..]);
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_bytes_bytesbuf");
    group.bench_function("put_bytes_bytesbuf", |b| {
        b.iter_batched_ref(
            BytesBuf::new,
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_bytes(test_data_as_seq.clone());
            },
            BatchSize::SmallInput,
        );
    });

    // Note: bytes crate doesn't have an equivalent to put_bytes(BytesView),
    // so we skip that comparison

    let allocs_op = allocs.operation("put_byte_bytesbuf");
    group.bench_function("put_byte_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(1, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_byte(black_box(0xAB));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u8_bytes");
    group.bench_function("put_u8_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(1, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u8(black_box(0xAB));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    // ============================================================================
    // GET_NUM operations (fast numeric reads)
    // ============================================================================

    let mut group = c.benchmark_group("get_num");

    // u8 - no endianness variants needed
    let allocs_op = allocs.operation("get_u8_bytesbuf");
    group.bench_function("get_u8_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<u8>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u8_bytes");
    group.bench_function("get_u8_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u8());
            },
            BatchSize::SmallInput,
        );
    });

    // i8 - no endianness variants needed
    let allocs_op = allocs.operation("get_i8_bytesbuf");
    group.bench_function("get_i8_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<i8>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i8_bytes");
    group.bench_function("get_i8_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i8());
            },
            BatchSize::SmallInput,
        );
    });

    // u16 - little-endian
    let allocs_op = allocs.operation("get_u16_le_bytesbuf");
    group.bench_function("get_u16_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<u16>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u16_le_bytes");
    group.bench_function("get_u16_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u16_le());
            },
            BatchSize::SmallInput,
        );
    });

    // u16 - big-endian
    let allocs_op = allocs.operation("get_u16_be_bytesbuf");
    group.bench_function("get_u16_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<u16>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u16_be_bytes");
    group.bench_function("get_u16_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u16());
            },
            BatchSize::SmallInput,
        );
    });

    // i16 - little-endian
    let allocs_op = allocs.operation("get_i16_le_bytesbuf");
    group.bench_function("get_i16_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<i16>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i16_le_bytes");
    group.bench_function("get_i16_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i16_le());
            },
            BatchSize::SmallInput,
        );
    });

    // i16 - big-endian
    let allocs_op = allocs.operation("get_i16_be_bytesbuf");
    group.bench_function("get_i16_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<i16>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i16_be_bytes");
    group.bench_function("get_i16_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i16());
            },
            BatchSize::SmallInput,
        );
    });

    // u32 - little-endian
    let allocs_op = allocs.operation("get_u32_le_bytesbuf");
    group.bench_function("get_u32_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<u32>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u32_le_bytes");
    group.bench_function("get_u32_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u32_le());
            },
            BatchSize::SmallInput,
        );
    });

    // u32 - big-endian
    let allocs_op = allocs.operation("get_u32_be_bytesbuf");
    group.bench_function("get_u32_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<u32>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u32_be_bytes");
    group.bench_function("get_u32_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u32());
            },
            BatchSize::SmallInput,
        );
    });

    // i32 - little-endian
    let allocs_op = allocs.operation("get_i32_le_bytesbuf");
    group.bench_function("get_i32_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<i32>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i32_le_bytes");
    group.bench_function("get_i32_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i32_le());
            },
            BatchSize::SmallInput,
        );
    });

    // i32 - big-endian
    let allocs_op = allocs.operation("get_i32_be_bytesbuf");
    group.bench_function("get_i32_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<i32>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i32_be_bytes");
    group.bench_function("get_i32_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i32());
            },
            BatchSize::SmallInput,
        );
    });

    // u64 - little-endian
    let allocs_op = allocs.operation("get_u64_le_bytesbuf");
    group.bench_function("get_u64_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<u64>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u64_le_bytes");
    group.bench_function("get_u64_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u64_le());
            },
            BatchSize::SmallInput,
        );
    });

    // u64 - big-endian
    let allocs_op = allocs.operation("get_u64_be_bytesbuf");
    group.bench_function("get_u64_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<u64>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_u64_be_bytes");
    group.bench_function("get_u64_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_u64());
            },
            BatchSize::SmallInput,
        );
    });

    // i64 - little-endian
    let allocs_op = allocs.operation("get_i64_le_bytesbuf");
    group.bench_function("get_i64_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<i64>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i64_le_bytes");
    group.bench_function("get_i64_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i64_le());
            },
            BatchSize::SmallInput,
        );
    });

    // i64 - big-endian
    let allocs_op = allocs.operation("get_i64_be_bytesbuf");
    group.bench_function("get_i64_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<i64>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_i64_be_bytes");
    group.bench_function("get_i64_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_i64());
            },
            BatchSize::SmallInput,
        );
    });

    // f32 - little-endian
    let allocs_op = allocs.operation("get_f32_le_bytesbuf");
    group.bench_function("get_f32_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<f32>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_f32_le_bytes");
    group.bench_function("get_f32_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_f32_le());
            },
            BatchSize::SmallInput,
        );
    });

    // f32 - big-endian
    let allocs_op = allocs.operation("get_f32_be_bytesbuf");
    group.bench_function("get_f32_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<f32>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_f32_be_bytes");
    group.bench_function("get_f32_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_f32());
            },
            BatchSize::SmallInput,
        );
    });

    // f64 - little-endian
    let allocs_op = allocs.operation("get_f64_le_bytesbuf");
    group.bench_function("get_f64_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_le::<f64>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_f64_le_bytes");
    group.bench_function("get_f64_le_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_f64_le());
            },
            BatchSize::SmallInput,
        );
    });

    // f64 - big-endian
    let allocs_op = allocs.operation("get_f64_be_bytesbuf");
    group.bench_function("get_f64_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_num_be::<f64>());
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("get_f64_be_bytes");
    group.bench_function("get_f64_be_bytes", |b| {
        b.iter_batched_ref(
            || many_as_seq.clone(),
            |seq| {
                let _span = allocs_op.measure_thread();
                black_box(seq.get_f64());
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    // ============================================================================
    // PUT_NUM operations (fast numeric writes)
    // ============================================================================

    let mut group = c.benchmark_group("put_num");

    // u8 - no endianness variants needed
    let allocs_op = allocs.operation("put_u8_bytesbuf");
    group.bench_function("put_u8_bytesbuf", |b| {
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

    let allocs_op = allocs.operation("put_u8_bytes");
    group.bench_function("put_u8_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(1, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u8(black_box(0xAB));
            },
            BatchSize::SmallInput,
        );
    });

    // i8 - no endianness variants needed
    let allocs_op = allocs.operation("put_i8_bytesbuf");
    group.bench_function("put_i8_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(1, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<i8>(black_box(-42));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i8_bytes");
    group.bench_function("put_i8_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(1, &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i8(black_box(-42));
            },
            BatchSize::SmallInput,
        );
    });

    // u16 - little-endian
    let allocs_op = allocs.operation("put_u16_le_bytesbuf");
    group.bench_function("put_u16_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<u16>(black_box(0x1234));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u16_le_bytes");
    group.bench_function("put_u16_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u16_le(black_box(0x1234));
            },
            BatchSize::SmallInput,
        );
    });

    // u16 - big-endian
    let allocs_op = allocs.operation("put_u16_be_bytesbuf");
    group.bench_function("put_u16_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<u16>(black_box(0x1234));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u16_be_bytes");
    group.bench_function("put_u16_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u16(black_box(0x1234));
            },
            BatchSize::SmallInput,
        );
    });

    // i16 - little-endian
    let allocs_op = allocs.operation("put_i16_le_bytesbuf");
    group.bench_function("put_i16_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<i16>(black_box(-1234));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i16_le_bytes");
    group.bench_function("put_i16_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i16_le(black_box(-1234));
            },
            BatchSize::SmallInput,
        );
    });

    // i16 - big-endian
    let allocs_op = allocs.operation("put_i16_be_bytesbuf");
    group.bench_function("put_i16_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<i16>(black_box(-1234));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i16_be_bytes");
    group.bench_function("put_i16_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i16>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i16(black_box(-1234));
            },
            BatchSize::SmallInput,
        );
    });

    // u32 - little-endian
    let allocs_op = allocs.operation("put_u32_le_bytesbuf");
    group.bench_function("put_u32_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<u32>(black_box(0x1234_5678));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u32_le_bytes");
    group.bench_function("put_u32_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u32_le(black_box(0x1234_5678));
            },
            BatchSize::SmallInput,
        );
    });

    // u32 - big-endian
    let allocs_op = allocs.operation("put_u32_be_bytesbuf");
    group.bench_function("put_u32_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<u32>(black_box(0x1234_5678));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u32_be_bytes");
    group.bench_function("put_u32_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u32(black_box(0x1234_5678));
            },
            BatchSize::SmallInput,
        );
    });

    // i32 - little-endian
    let allocs_op = allocs.operation("put_i32_le_bytesbuf");
    group.bench_function("put_i32_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<i32>(black_box(-123_456));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i32_le_bytes");
    group.bench_function("put_i32_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i32_le(black_box(-123_456));
            },
            BatchSize::SmallInput,
        );
    });

    // i32 - big-endian
    let allocs_op = allocs.operation("put_i32_be_bytesbuf");
    group.bench_function("put_i32_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<i32>(black_box(-123_456));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i32_be_bytes");
    group.bench_function("put_i32_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i32(black_box(-123_456));
            },
            BatchSize::SmallInput,
        );
    });

    // u64 - little-endian
    let allocs_op = allocs.operation("put_u64_le_bytesbuf");
    group.bench_function("put_u64_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<u64>(black_box(0x1234_5678_9ABC_DEF0));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_u64_le_bytes");
    group.bench_function("put_u64_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u64_le(black_box(0x1234_5678_9ABC_DEF0));
            },
            BatchSize::SmallInput,
        );
    });

    // u64 - big-endian
    let allocs_op = allocs.operation("put_u64_be_bytesbuf");
    group.bench_function("put_u64_be_bytesbuf", |b| {
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

    let allocs_op = allocs.operation("put_u64_be_bytes");
    group.bench_function("put_u64_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<u64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_u64(black_box(0x1234_5678_9ABC_DEF0));
            },
            BatchSize::SmallInput,
        );
    });

    // i64 - little-endian
    let allocs_op = allocs.operation("put_i64_le_bytesbuf");
    group.bench_function("put_i64_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<i64>(black_box(-1_234_567_890));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i64_le_bytes");
    group.bench_function("put_i64_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i64_le(black_box(-1_234_567_890));
            },
            BatchSize::SmallInput,
        );
    });

    // i64 - big-endian
    let allocs_op = allocs.operation("put_i64_be_bytesbuf");
    group.bench_function("put_i64_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<i64>(black_box(-1_234_567_890));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_i64_be_bytes");
    group.bench_function("put_i64_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<i64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_i64(black_box(-1_234_567_890));
            },
            BatchSize::SmallInput,
        );
    });

    // f32 - little-endian
    let allocs_op = allocs.operation("put_f32_le_bytesbuf");
    group.bench_function("put_f32_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<f32>(black_box(123.456));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_f32_le_bytes");
    group.bench_function("put_f32_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_f32_le(black_box(123.456));
            },
            BatchSize::SmallInput,
        );
    });

    // f32 - big-endian
    let allocs_op = allocs.operation("put_f32_be_bytesbuf");
    group.bench_function("put_f32_be_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_be::<f32>(black_box(123.456));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_f32_be_bytes");
    group.bench_function("put_f32_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f32>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_f32(black_box(123.456));
            },
            BatchSize::SmallInput,
        );
    });

    // f64 - little-endian
    let allocs_op = allocs.operation("put_f64_le_bytesbuf");
    group.bench_function("put_f64_le_bytesbuf", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_num_le::<f64>(black_box(1234.5678));
            },
            BatchSize::SmallInput,
        );
    });

    let allocs_op = allocs.operation("put_f64_le_bytes");
    group.bench_function("put_f64_le_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_f64_le(black_box(1234.5678));
            },
            BatchSize::SmallInput,
        );
    });

    // f64 - big-endian
    let allocs_op = allocs.operation("put_f64_be_bytesbuf");
    group.bench_function("put_f64_be_bytesbuf", |b| {
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

    let allocs_op = allocs.operation("put_f64_be_bytes");
    group.bench_function("put_f64_be_bytes", |b| {
        b.iter_batched_ref(
            || {
                let mut sb = BytesBuf::new();
                sb.reserve(std::mem::size_of::<f64>(), &transparent_memory);
                sb
            },
            |sb| {
                let _span = allocs_op.measure_thread();
                sb.put_f64(black_box(1234.5678));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();

    allocs.print_to_stdout();
}
