// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;
use std::hint::black_box;
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::time::Instant;

use alloc_tracker::{Allocator, Session};
use bytes::{Buf, BufMut};
use bytesbuf::{BlockSize, BytesBuf, BytesView, FixedBlockTestMemory, TransparentTestMemory};
use criterion::{Criterion, criterion_group, criterion_main};
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

    let mut group = c.benchmark_group("bytesbuf_vs_bytes_get");

    // get_byte (bytesbuf) vs get_u8 (bytes)
    let allocs_op = allocs.operation("get_byte");
    group.bench_function("get_byte", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_byte());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u8");
    group.bench_function("get_u8", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u8());
            }
            start.elapsed()
        });
    });

    // copy_to_slice (both use same method, but we benchmark both to prove equivalence)
    // Also includes copy_to_uninit_slice as a related operation
    let allocs_op = allocs.operation("copy_to_slice");
    group.bench_function("copy_to_slice", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences and target buffers outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();
            let mut targets: Vec<_> = (0..iters).map(|_| [0u8; COPY_TO_SLICE_LEN]).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (seq, target) in sequences.iter_mut().zip(targets.iter_mut()) {
                seq.copy_to_slice(target);
                black_box(target);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("copy_to_uninit_slice");
    group.bench_function("copy_to_uninit_slice", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences and target buffers outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();
            let mut targets: Vec<_> = (0..iters)
                .map(|_| [MaybeUninit::<u8>::uninit(); COPY_TO_SLICE_LEN])
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (seq, target) in sequences.iter_mut().zip(targets.iter_mut()) {
                seq.copy_to_uninit_slice(target);
                black_box(target);
            }
            start.elapsed()
        });
    });

    group.finish();

    // ============================================================================
    // PUT operations (slower, non-numeric)
    // ============================================================================

    let mut group = c.benchmark_group("bytesbuf_vs_bytes_put");

    // put_slice - both use same method from BufMut trait
    let allocs_op = allocs.operation("put_slice");
    group.bench_function("put_slice", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers and data outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(COPY_TO_SLICE_LEN, &transparent_memory);
                    sb
                })
                .collect();
            let data = [0xCD_u8; COPY_TO_SLICE_LEN];

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_slice(&data[..]);
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // put_bytes (BytesView) - unique to bytesbuf, no bytes equivalent
    let allocs_op = allocs.operation("put_bytes_view");
    group.bench_function("put_bytes_view", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters).map(|_| BytesBuf::new()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_bytes(test_data_as_seq.clone());
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // put_byte (bytesbuf) vs put_u8 (bytes)
    let allocs_op = allocs.operation("put_byte");
    group.bench_function("put_byte", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(1, &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_byte(black_box(0xAB));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u8");
    group.bench_function("put_u8", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(1, &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u8(black_box(0xAB));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // put_byte_repeated (bytesbuf) vs put_bytes (bytes) - these are equivalent!
    let allocs_op = allocs.operation("put_byte_repeated");
    group.bench_function("put_byte_repeated", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(COPY_TO_SLICE_LEN, &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_byte_repeated(black_box(0xCD), COPY_TO_SLICE_LEN);
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_bytes");
    group.bench_function("put_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(COPY_TO_SLICE_LEN, &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                BufMut::put_bytes(sb, black_box(0xCD), COPY_TO_SLICE_LEN);
                black_box(sb);
            }
            start.elapsed()
        });
    });

    group.finish();

    // ============================================================================
    // GET_NUM operations (fast numeric reads)
    // ============================================================================

    let mut group = c.benchmark_group("bytesbuf_vs_bytes_get_num");

    // u8
    let allocs_op = allocs.operation("get_u8");
    group.bench_function("get_u8", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<u8>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u8_bytes");
    group.bench_function("get_u8_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u8());
            }
            start.elapsed()
        });
    });

    // i8
    let allocs_op = allocs.operation("get_i8");
    group.bench_function("get_i8", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<i8>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i8_bytes");
    group.bench_function("get_i8_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i8());
            }
            start.elapsed()
        });
    });

    // u16 little-endian
    let allocs_op = allocs.operation("get_u16_le");
    group.bench_function("get_u16_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<u16>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u16_le_bytes");
    group.bench_function("get_u16_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u16_le());
            }
            start.elapsed()
        });
    });

    // u16 big-endian
    let allocs_op = allocs.operation("get_u16_be");
    group.bench_function("get_u16_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<u16>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u16_be_bytes");
    group.bench_function("get_u16_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u16());
            }
            start.elapsed()
        });
    });

    // i16 little-endian
    let allocs_op = allocs.operation("get_i16_le");
    group.bench_function("get_i16_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<i16>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i16_le_bytes");
    group.bench_function("get_i16_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i16_le());
            }
            start.elapsed()
        });
    });

    // i16 big-endian
    let allocs_op = allocs.operation("get_i16_be");
    group.bench_function("get_i16_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<i16>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i16_be_bytes");
    group.bench_function("get_i16_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i16());
            }
            start.elapsed()
        });
    });

    // u32 little-endian
    let allocs_op = allocs.operation("get_u32_le");
    group.bench_function("get_u32_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<u32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u32_le_bytes");
    group.bench_function("get_u32_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u32_le());
            }
            start.elapsed()
        });
    });

    // u32 big-endian
    let allocs_op = allocs.operation("get_u32_be");
    group.bench_function("get_u32_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<u32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u32_be_bytes");
    group.bench_function("get_u32_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u32());
            }
            start.elapsed()
        });
    });

    // i32 little-endian
    let allocs_op = allocs.operation("get_i32_le");
    group.bench_function("get_i32_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<i32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i32_le_bytes");
    group.bench_function("get_i32_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i32_le());
            }
            start.elapsed()
        });
    });

    // i32 big-endian
    let allocs_op = allocs.operation("get_i32_be");
    group.bench_function("get_i32_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<i32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i32_be_bytes");
    group.bench_function("get_i32_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i32());
            }
            start.elapsed()
        });
    });

    // u64 little-endian
    let allocs_op = allocs.operation("get_u64_le");
    group.bench_function("get_u64_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<u64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u64_le_bytes");
    group.bench_function("get_u64_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u64_le());
            }
            start.elapsed()
        });
    });

    // u64 big-endian
    let allocs_op = allocs.operation("get_u64_be");
    group.bench_function("get_u64_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<u64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u64_be_bytes");
    group.bench_function("get_u64_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_u64());
            }
            start.elapsed()
        });
    });

    // i64 little-endian
    let allocs_op = allocs.operation("get_i64_le");
    group.bench_function("get_i64_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<i64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i64_le_bytes");
    group.bench_function("get_i64_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i64_le());
            }
            start.elapsed()
        });
    });

    // i64 big-endian
    let allocs_op = allocs.operation("get_i64_be");
    group.bench_function("get_i64_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<i64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_i64_be_bytes");
    group.bench_function("get_i64_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_i64());
            }
            start.elapsed()
        });
    });

    // f32 little-endian
    let allocs_op = allocs.operation("get_f32_le");
    group.bench_function("get_f32_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<f32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f32_le_bytes");
    group.bench_function("get_f32_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_f32_le());
            }
            start.elapsed()
        });
    });

    // f32 big-endian
    let allocs_op = allocs.operation("get_f32_be");
    group.bench_function("get_f32_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<f32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f32_be_bytes");
    group.bench_function("get_f32_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_f32());
            }
            start.elapsed()
        });
    });

    // f64 little-endian
    let allocs_op = allocs.operation("get_f64_le");
    group.bench_function("get_f64_le", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_le::<f64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f64_le_bytes");
    group.bench_function("get_f64_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_f64_le());
            }
            start.elapsed()
        });
    });

    // f64 big-endian
    let allocs_op = allocs.operation("get_f64_be");
    group.bench_function("get_f64_be", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_num_be::<f64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f64_be_bytes");
    group.bench_function("get_f64_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare sequences outside the timed loop
            let mut sequences: Vec<_> = (0..iters).map(|_| many_as_seq.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for seq in &mut sequences {
                black_box(seq.get_f64());
            }
            start.elapsed()
        });
    });

    group.finish();

    // ============================================================================
    // PUT_NUM operations (fast numeric writes)
    // ============================================================================

    let mut group = c.benchmark_group("bytesbuf_vs_bytes_put_num");

    // u8
    let allocs_op = allocs.operation("put_u8");
    group.bench_function("put_u8", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u8>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<u8>(black_box(0xAB));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u8_bytes");
    group.bench_function("put_u8_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u8>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u8(black_box(0xAB));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i8
    let allocs_op = allocs.operation("put_i8");
    group.bench_function("put_i8", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i8>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<i8>(black_box(-42));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i8_bytes");
    group.bench_function("put_i8_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i8>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i8(black_box(-42));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // u16 little-endian
    let allocs_op = allocs.operation("put_u16_le");
    group.bench_function("put_u16_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<u16>(black_box(0x1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u16_le_bytes");
    group.bench_function("put_u16_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u16_le(black_box(0x1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // u16 big-endian
    let allocs_op = allocs.operation("put_u16_be");
    group.bench_function("put_u16_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<u16>(black_box(0x1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u16_be_bytes");
    group.bench_function("put_u16_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u16(black_box(0x1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i16 little-endian
    let allocs_op = allocs.operation("put_i16_le");
    group.bench_function("put_i16_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<i16>(black_box(-1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i16_le_bytes");
    group.bench_function("put_i16_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i16_le(black_box(-1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i16 big-endian
    let allocs_op = allocs.operation("put_i16_be");
    group.bench_function("put_i16_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<i16>(black_box(-1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i16_be_bytes");
    group.bench_function("put_i16_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i16>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i16(black_box(-1234));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // u32 little-endian
    let allocs_op = allocs.operation("put_u32_le");
    group.bench_function("put_u32_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<u32>(black_box(0x1234_5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u32_le_bytes");
    group.bench_function("put_u32_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u32_le(black_box(0x1234_5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // u32 big-endian
    let allocs_op = allocs.operation("put_u32_be");
    group.bench_function("put_u32_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<u32>(black_box(0x1234_5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u32_be_bytes");
    group.bench_function("put_u32_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u32(black_box(0x1234_5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i32 little-endian
    let allocs_op = allocs.operation("put_i32_le");
    group.bench_function("put_i32_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<i32>(black_box(-123_456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i32_le_bytes");
    group.bench_function("put_i32_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i32_le(black_box(-123_456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i32 big-endian
    let allocs_op = allocs.operation("put_i32_be");
    group.bench_function("put_i32_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<i32>(black_box(-123_456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i32_be_bytes");
    group.bench_function("put_i32_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i32(black_box(-123_456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // u64 little-endian
    let allocs_op = allocs.operation("put_u64_le");
    group.bench_function("put_u64_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<u64>(black_box(0x1234_5678_9ABC_DEF0));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u64_le_bytes");
    group.bench_function("put_u64_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u64_le(black_box(0x1234_5678_9ABC_DEF0));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // u64 big-endian
    let allocs_op = allocs.operation("put_u64_be");
    group.bench_function("put_u64_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<u64>(black_box(0x1234_5678_9ABC_DEF0));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u64_be_bytes");
    group.bench_function("put_u64_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<u64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_u64(black_box(0x1234_5678_9ABC_DEF0));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i64 little-endian
    let allocs_op = allocs.operation("put_i64_le");
    group.bench_function("put_i64_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<i64>(black_box(-1_234_567_890));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i64_le_bytes");
    group.bench_function("put_i64_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i64_le(black_box(-1_234_567_890));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // i64 big-endian
    let allocs_op = allocs.operation("put_i64_be");
    group.bench_function("put_i64_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<i64>(black_box(-1_234_567_890));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_i64_be_bytes");
    group.bench_function("put_i64_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<i64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_i64(black_box(-1_234_567_890));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // f32 little-endian
    let allocs_op = allocs.operation("put_f32_le");
    group.bench_function("put_f32_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<f32>(black_box(123.456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f32_le_bytes");
    group.bench_function("put_f32_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_f32_le(black_box(123.456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // f32 big-endian
    let allocs_op = allocs.operation("put_f32_be");
    group.bench_function("put_f32_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<f32>(black_box(123.456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f32_be_bytes");
    group.bench_function("put_f32_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f32>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_f32(black_box(123.456));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // f64 little-endian
    let allocs_op = allocs.operation("put_f64_le");
    group.bench_function("put_f64_le", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_le::<f64>(black_box(1234.5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f64_le_bytes");
    group.bench_function("put_f64_le_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_f64_le(black_box(1234.5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    // f64 big-endian
    let allocs_op = allocs.operation("put_f64_be");
    group.bench_function("put_f64_be", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_num_be::<f64>(black_box(1234.5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f64_be_bytes");
    group.bench_function("put_f64_be_bytes", |b| {
        b.iter_custom(|iters| {
            // Prepare buffers outside the timed loop
            let mut buffers: Vec<_> = (0..iters)
                .map(|_| {
                    let mut sb = BytesBuf::new();
                    sb.reserve(size_of::<f64>(), &transparent_memory);
                    sb
                })
                .collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for sb in &mut buffers {
                sb.put_f64(black_box(1234.5678));
                black_box(sb);
            }
            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
