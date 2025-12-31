// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::f64;
use std::hint::black_box;
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::time::Instant;
use std::{alloc::System, iter};

use alloc_tracker::{Allocator, Session};
use bytes::{Buf, BufMut};
use bytesbuf::{BlockSize, BytesBuf, BytesView, TransparentTestMemory};
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
const WORKING_SLICE_LEN: usize = 256;

#[expect(clippy::too_many_lines, reason = "Is fine - lots of benchmarks to do!")]
fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let memory = TransparentTestMemory::new();

    let test_data_view = BytesView::copied_from_slice(TEST_DATA, &memory);
    let many = iter::repeat_n(test_data_view.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_view = BytesView::from_views(many.iter().cloned());

    let mut group = c.benchmark_group("bytesbuf_vs_copy_out");

    let allocs_op = allocs.operation("slice");
    group.bench_function("slice", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();
            let mut targets: Vec<_> = (0..iters).map(|_| [0u8; WORKING_SLICE_LEN]).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (bytes, target) in inputs.iter_mut().zip(targets.iter_mut()) {
                bytes.copy_to_slice(target);
                black_box(target);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("slice_bytes");
    group.bench_function("slice_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();
            let mut targets: Vec<_> = (0..iters).map(|_| [0u8; WORKING_SLICE_LEN]).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (bytes, target) in inputs.iter_mut().zip(targets.iter_mut()) {
                Buf::copy_to_slice(bytes, target);
                black_box(target);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("uninit_slice");
    group.bench_function("uninit_slice", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();
            let mut targets: Vec<_> = (0..iters).map(|_| [MaybeUninit::<u8>::uninit(); WORKING_SLICE_LEN]).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for (bytes, target) in inputs.iter_mut().zip(targets.iter_mut()) {
                bytes.copy_to_uninit_slice(target);
                black_box(target);
            }
            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_copy_in");

    let allocs_op = allocs.operation("put_slice");
    group.bench_function("put_slice", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(WORKING_SLICE_LEN)).collect();
            let data = [0xCD_u8; WORKING_SLICE_LEN];

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_slice(data);
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_slice_bytes");
    group.bench_function("put_slice_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(WORKING_SLICE_LEN)).collect();
            let data = [0xCD_u8; WORKING_SLICE_LEN];

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put(buf, &data[..]);
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_bytes_view");
    group.bench_function("put_bytes_view", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| BytesBuf::new()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_bytes(test_data_view.clone());
                black_box(buf);
            }
            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_u8");

    let allocs_op = allocs.operation("put_byte");
    group.bench_function("put_byte", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(1)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_byte(black_box(0xAB));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_byte_bytes");
    group.bench_function("put_byte_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(1)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u8(buf, black_box(0xAB));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u8");
    group.bench_function("put_u8", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(1)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_ne::<u8>(black_box(0xAB));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_u8_repeated");

    let allocs_op = allocs.operation("put_byte_repeated");
    group.bench_function("put_byte_repeated", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(WORKING_SLICE_LEN)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_byte_repeated(black_box(0xCD), WORKING_SLICE_LEN);
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_bytes");
    group.bench_function("put_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(WORKING_SLICE_LEN)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_bytes(buf, black_box(0xCD), WORKING_SLICE_LEN);
                black_box(buf);
            }
            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_num");

    let allocs_op = allocs.operation("put_u16_le");
    group.bench_function("put_u16_le", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(2)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_le::<u16>(black_box(0xABCD));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u16_le_bytes");
    group.bench_function("put_u16_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(2)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u16_le(buf, black_box(0xABCD));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u32_le");
    group.bench_function("put_u32_le", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(4)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_le::<u32>(black_box(0xABCD_EF01));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u32_le_bytes");
    group.bench_function("put_u32_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(4)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u32_le(buf, black_box(0xABCD_EF01));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u64_le");
    group.bench_function("put_u64_le", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_le::<u64>(black_box(0xABCD_EF01_2345_6789));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u64_le_bytes");
    group.bench_function("put_u64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u64_le(buf, black_box(0xABCD_EF01_2345_6789));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f64_le");
    group.bench_function("put_f64_le", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_le::<f64>(black_box(f64::consts::PI));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f64_le_bytes");
    group.bench_function("put_f64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_f64_le(buf, black_box(f64::consts::PI));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u16_be");
    group.bench_function("put_u16_be", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(2)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_be::<u16>(black_box(0xABCD));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u16_be_bytes");
    group.bench_function("put_u16_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(2)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u16(buf, black_box(0xABCD));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u32_be");
    group.bench_function("put_u32_be", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(4)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_be::<u32>(black_box(0xABCD_EF01));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u32_be_bytes");
    group.bench_function("put_u32_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(4)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u32(buf, black_box(0xABCD_EF01));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u64_be");
    group.bench_function("put_u64_be", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_be::<u64>(black_box(0xABCD_EF01_2345_6789));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_u64_be_bytes");
    group.bench_function("put_u64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_u64(buf, black_box(0xABCD_EF01_2345_6789));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f64_be");
    group.bench_function("put_f64_be", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                buf.put_num_be::<f64>(black_box(f64::consts::PI));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("put_f64_be_bytes");
    group.bench_function("put_f64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut buffers: Vec<_> = (0..iters).map(|_| memory.reserve(8)).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for buf in &mut buffers {
                BufMut::put_f64(buf, black_box(f64::consts::PI));
                black_box(buf);
            }
            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_u8");

    // get_byte is a "manual specialization" of get_num_le::<u8>()
    let allocs_op = allocs.operation("get_byte");
    group.bench_function("get_byte", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_byte());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u8");
    group.bench_function("get_u8", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_le::<u8>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u8_bytes");
    group.bench_function("get_u8_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_u8());
            }
            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_get_num");

    let allocs_op = allocs.operation("get_u16_le");
    group.bench_function("get_u16_le", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_le::<u16>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u16_le_bytes");
    group.bench_function("get_u16_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_u16_le(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u32_le");
    group.bench_function("get_u32_le", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_le::<u32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u32_le_bytes");
    group.bench_function("get_u32_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_u32_le(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u64_le");
    group.bench_function("get_u64_le", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_le::<u64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u64_le_bytes");
    group.bench_function("get_u64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_u64_le(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f64_le");
    group.bench_function("get_f64_le", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_le::<f64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f64_le_bytes");
    group.bench_function("get_f64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_f64_le(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u16_be");
    group.bench_function("get_u16_be", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_be::<u16>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u16_be_bytes");
    group.bench_function("get_u16_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_u16(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u32_be");
    group.bench_function("get_u32_be", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_be::<u32>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u32_be_bytes");
    group.bench_function("get_u32_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_u32(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u64_be");
    group.bench_function("get_u64_be", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_be::<u64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_u64_be_bytes");
    group.bench_function("get_u64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_u64(bytes));
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f64_be");
    group.bench_function("get_f64_be", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(bytes.get_num_be::<f64>());
            }
            start.elapsed()
        });
    });

    let allocs_op = allocs.operation("get_f64_be_bytes");
    group.bench_function("get_f64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let mut inputs: Vec<_> = (0..iters).map(|_| many_as_view.clone()).collect();

            let _span = allocs_op.measure_thread().iterations(iters);
            let start = Instant::now();
            for bytes in &mut inputs {
                black_box(Buf::get_f64(bytes));
            }
            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
