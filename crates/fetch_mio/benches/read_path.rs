// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compares the former buffered read path with direct destination reads.

#![allow(missing_docs, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

const GROUP: &str = "read_path/read";
const INTERMEDIATE_BUFFER_SIZE: usize = 16 * 1_024;
const SIZES: [(&str, usize); 3] = [("1_kib", 1_024), ("16_kib", 16 * 1_024), ("256_kib", 256 * 1_024)];

struct ReadInput {
    source: Vec<u8>,
    destination: Vec<u8>,
}

struct ReadOutput {
    input: ReadInput,
    extra_copied_bytes: usize,
}

fn input(size: usize) -> ReadInput {
    ReadInput {
        source: (0_u8..=u8::MAX).cycle().take(size).collect(),
        destination: vec![0; size],
    }
}

#[metabench::benchmark(BUFFERED, GROUP, "buffered")]
#[bench::bytes_1_kib(input(1_024))]
#[bench::bytes_16_kib(input(16 * 1_024))]
#[bench::bytes_256_kib(input(256 * 1_024))]
fn buffered(mut input: ReadInput) -> ReadOutput {
    let mut intermediate = vec![0_u8; INTERMEDIATE_BUFFER_SIZE].into_boxed_slice();
    let mut offset = 0;

    while offset < input.source.len() {
        let length = (input.source.len() - offset).min(intermediate.len());
        intermediate[..length].copy_from_slice(&input.source[offset..offset + length]);
        input.destination[offset..offset + length].copy_from_slice(&intermediate[..length]);
        offset += length;
    }

    black_box(intermediate);
    assert_eq!(input.destination, input.source);
    let extra_copied_bytes = input.source.len();
    ReadOutput { input, extra_copied_bytes }
}

#[metabench::benchmark(DIRECT, GROUP, "direct")]
#[bench::bytes_1_kib(input(1_024))]
#[bench::bytes_16_kib(input(16 * 1_024))]
#[bench::bytes_256_kib(input(256 * 1_024))]
fn direct(mut input: ReadInput) -> ReadOutput {
    input.destination.copy_from_slice(&input.source);
    assert_eq!(input.destination, input.source);
    ReadOutput {
        input,
        extra_copied_bytes: 0,
    }
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(GROUP);

    for (case, size) in SIZES {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new(BUFFERED.benchmark_name(), case), &size, |bencher, size| {
            bencher.iter_batched(
                || input(*size),
                |input| {
                    let output = buffered(input);
                    black_box(output.extra_copied_bytes);
                    output.input
                },
                BatchSize::PerIteration,
            );
        });
        group.bench_with_input(BenchmarkId::new(DIRECT.benchmark_name(), case), &size, |bencher, size| {
            bencher.iter_batched(
                || input(*size),
                |input| {
                    let output = direct(input);
                    black_box(output.extra_copied_bytes);
                    output.input
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [BUFFERED, DIRECT],
    allocator = std::alloc::System,
);
