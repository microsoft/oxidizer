// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Benchmark code")]

use std::alloc::System;
use std::hint::black_box;
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::{f64, iter};

use alloc_tracker::{Allocator, Session};
use benchmarking::time_sample_with_inputs;
use bytes::{Buf, BufMut};
use bytesbuf::mem::BlockSize;
use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::{BytesBuf, BytesView};
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

fn prepared_inputs<T>(iters: u64, mut setup: impl FnMut() -> T) -> Vec<T> {
    (0..iters).map(|_| setup()).collect()
}

#[expect(clippy::too_many_lines, reason = "Is fine - lots of benchmarks to do!")]
fn entrypoint(c: &mut Criterion) {
    let allocs = Session::new();

    let memory = TransparentMemory::new();

    let test_data_view = BytesView::copied_from_slice(TEST_DATA, &memory);
    let many = iter::repeat_n(test_data_view.clone(), MANY_SPANS).collect::<Vec<_>>();
    let many_as_view = BytesView::from_views(many.iter().cloned());

    let mut group = c.benchmark_group("bytesbuf_vs_copy_out");

    let allocs_op = allocs.operation("slice");
    group.bench_function("slice", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || (many_as_view.clone(), [0u8; WORKING_SLICE_LEN]));
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |(bytes, target)| {
                    bytes.copy_to_slice(target);
                    black_box(target);
                },
            )
        });
    });

    let allocs_op = allocs.operation("slice_bytes");
    group.bench_function("slice_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || (many_as_view.clone(), [0u8; WORKING_SLICE_LEN]));
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |(bytes, target)| {
                    Buf::copy_to_slice(bytes, target);
                    black_box(target);
                },
            )
        });
    });

    let allocs_op = allocs.operation("uninit_slice");
    group.bench_function("uninit_slice", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || (many_as_view.clone(), [MaybeUninit::<u8>::uninit(); WORKING_SLICE_LEN]));
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |(bytes, target)| {
                    bytes.copy_to_uninit_slice(target);
                    black_box(target);
                },
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_copy_in");

    let allocs_op = allocs.operation("put_slice");
    group.bench_function("put_slice", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(WORKING_SLICE_LEN));
            let data = [0xCD_u8; WORKING_SLICE_LEN];

            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_slice(data);
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_slice_bytes");
    group.bench_function("put_slice_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(WORKING_SLICE_LEN));
            let data = [0xCD_u8; WORKING_SLICE_LEN];

            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put(buf, &data[..]);
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_bytes_view");
    group.bench_function("put_bytes_view", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, BytesBuf::new);
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_bytes(test_data_view.clone());
                    black_box(buf);
                },
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_u8");

    let allocs_op = allocs.operation("put_byte");
    group.bench_function("put_byte", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(1));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_byte(black_box(0xAB));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_byte_bytes");
    group.bench_function("put_byte_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(1));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u8(buf, black_box(0xAB));
                    black_box(buf);
                },
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_u8_repeated");

    let allocs_op = allocs.operation("put_byte_repeated");
    group.bench_function("put_byte_repeated", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(WORKING_SLICE_LEN));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_byte_repeated(black_box(0xCD), WORKING_SLICE_LEN);
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_bytes");
    group.bench_function("put_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(WORKING_SLICE_LEN));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_bytes(buf, black_box(0xCD), WORKING_SLICE_LEN);
                    black_box(buf);
                },
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_num");

    let allocs_op = allocs.operation("put_u16_le");
    group.bench_function("put_u16_le", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(2));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_u16_le(black_box(0xABCD));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u16_le_bytes");
    group.bench_function("put_u16_le_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(2));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u16_le(buf, black_box(0xABCD));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u32_le");
    group.bench_function("put_u32_le", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(4));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_u32_le(black_box(0xABCD_EF01));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u32_le_bytes");
    group.bench_function("put_u32_le_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(4));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u32_le(buf, black_box(0xABCD_EF01));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u64_le");
    group.bench_function("put_u64_le", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_u64_le(black_box(0xABCD_EF01_2345_6789));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u64_le_bytes");
    group.bench_function("put_u64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u64_le(buf, black_box(0xABCD_EF01_2345_6789));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_f64_le");
    group.bench_function("put_f64_le", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_f64_le(black_box(f64::consts::PI));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_f64_le_bytes");
    group.bench_function("put_f64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_f64_le(buf, black_box(f64::consts::PI));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u16_be");
    group.bench_function("put_u16_be", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(2));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_u16_be(black_box(0xABCD));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u16_be_bytes");
    group.bench_function("put_u16_be_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(2));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u16(buf, black_box(0xABCD));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u32_be");
    group.bench_function("put_u32_be", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(4));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_u32_be(black_box(0xABCD_EF01));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u32_be_bytes");
    group.bench_function("put_u32_be_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(4));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u32(buf, black_box(0xABCD_EF01));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u64_be");
    group.bench_function("put_u64_be", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_u64_be(black_box(0xABCD_EF01_2345_6789));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_u64_be_bytes");
    group.bench_function("put_u64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_u64(buf, black_box(0xABCD_EF01_2345_6789));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_f64_be");
    group.bench_function("put_f64_be", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    buf.put_f64_be(black_box(f64::consts::PI));
                    black_box(buf);
                },
            )
        });
    });

    let allocs_op = allocs.operation("put_f64_be_bytes");
    group.bench_function("put_f64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let buffers = prepared_inputs(iters, || memory.reserve(8));
            time_sample_with_inputs(
                buffers,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |buf| {
                    BufMut::put_f64(buf, black_box(f64::consts::PI));
                    black_box(buf);
                },
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_put_u8");

    let allocs_op = allocs.operation("get_byte");
    group.bench_function("get_byte", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_byte());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u8_bytes");
    group.bench_function("get_u8_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u8());
                },
            )
        });
    });

    group.finish();

    let mut group = c.benchmark_group("bytesbuf_vs_get_num");

    let allocs_op = allocs.operation("get_u16_le");
    group.bench_function("get_u16_le", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u16_le());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u16_le_bytes");
    group.bench_function("get_u16_le_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_u16_le(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_u32_le");
    group.bench_function("get_u32_le", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u32_le());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u32_le_bytes");
    group.bench_function("get_u32_le_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_u32_le(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_u64_le");
    group.bench_function("get_u64_le", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u64_le());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u64_le_bytes");
    group.bench_function("get_u64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_u64_le(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_f64_le");
    group.bench_function("get_f64_le", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_f64_le());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_f64_le_bytes");
    group.bench_function("get_f64_le_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_f64_le(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_u16_be");
    group.bench_function("get_u16_be", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u16_be());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u16_be_bytes");
    group.bench_function("get_u16_be_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_u16(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_u32_be");
    group.bench_function("get_u32_be", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u32_be());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u32_be_bytes");
    group.bench_function("get_u32_be_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_u32(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_u64_be");
    group.bench_function("get_u64_be", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_u64_be());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_u64_be_bytes");
    group.bench_function("get_u64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_u64(bytes)),
            )
        });
    });

    let allocs_op = allocs.operation("get_f64_be");
    group.bench_function("get_f64_be", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| {
                    black_box(bytes.get_f64_be());
                },
            )
        });
    });

    let allocs_op = allocs.operation("get_f64_be_bytes");
    group.bench_function("get_f64_be_bytes", |b| {
        b.iter_custom(|iters| {
            let inputs = prepared_inputs(iters, || many_as_view.clone());
            time_sample_with_inputs(
                inputs,
                |sample_iters| allocs_op.measure_thread().iterations(sample_iters),
                |bytes| black_box(Buf::get_f64(bytes)),
            )
        });
    });

    group.finish();
}
