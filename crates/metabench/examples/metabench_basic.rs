// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One shared workload and input registered with Criterion and Gungraun.

use criterion::Criterion;

const CHECKSUM_GROUP: &str = "basic/checksum";

fn rolling_checksum(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0_u64, |checksum, byte| checksum.rotate_left(5) ^ u64::from(*byte))
}

#[metabench::benchmark(ROLLING, CHECKSUM_GROUP, "rolling")]
fn rolling_checksum_benchmark() -> u64 {
    let bytes = std::hint::black_box(vec![42; 4_096]);
    rolling_checksum(&bytes)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ROLLING.group_name());
    group.bench_function(ROLLING.benchmark_name(), |bencher| {
        bencher.iter(rolling_checksum_benchmark);
    });
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        CHECKSUMS {
            benchmarks = [ROLLING],
            gungraun_max_parallel = 1,
        },
    },
);
