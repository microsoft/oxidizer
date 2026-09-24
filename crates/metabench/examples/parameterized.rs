// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared cases and setup registered with both benchmark engines.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};

const SMALL: usize = 128;
const MEDIUM: usize = 1_024;
const LARGE: usize = 8_192;
const CASES: [(usize, &str); 3] = [(SMALL, "size_128"), (MEDIUM, "size_1024"), (LARGE, "size_8192")];

fn descending_values(size: usize) -> Vec<u64> {
    (0..size as u64).rev().collect()
}

fn sort(values: &mut [u64]) {
    values.sort_unstable();
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(SORT_UNSTABLE.group_name());
    for (size, case) in CASES {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new(SORT_UNSTABLE.benchmark_name(), case), &size, |bencher, &size| {
            bencher.iter_batched(|| descending_values(size), unstable_sort, BatchSize::SmallInput);
        });
    }
    group.finish();
}

#[metabench::benchmark(SORT_UNSTABLE, "parameterized/sort", "unstable")]
#[bench::size_128(descending_values(SMALL))]
#[bench::size_1024(descending_values(MEDIUM))]
#[bench::size_8192(descending_values(LARGE))]
fn unstable_sort(mut values: Vec<u64>) -> Vec<u64> {
    sort(&mut values);
    values
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [SORT_UNSTABLE],);
