// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Concurrent in-process throughput for five frameworks and one control.

#![allow(dead_code, reason = "the shared fixture supports the harness and regular tests")]
#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
use std::io::Write;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

include!("common/throughput_scenarios.rs");

/// How many fixed-count batches the standalone runner times per row.
const REPEATS: usize = 15;

fn print_environment(stderr: &mut impl Write) {
    let shape = shape();
    writeln!(
        stderr,
        "throughput shape: workers={} slots={} requests_per_slot={} requests_per_batch={} repeats={}",
        shape.workers,
        shape.slots,
        shape.requests_per_slot,
        shape.requests_per_batch(),
        REPEATS,
    )
    .expect("writing the throughput shape to stderr should succeed");
    writeln!(
        stderr,
        "throughput workloads: light={} rounds, heavy={} rounds; available parallelism={:?}",
        Workload::Light.rounds(),
        Workload::Heavy.rounds(),
        std::thread::available_parallelism().map(std::num::NonZero::get),
    )
    .expect("writing the throughput workloads to stderr should succeed");
}

fn print_standalone_runner(stderr: &mut impl Write) {
    for measurement in measure(REPEATS) {
        writeln!(
            stderr,
            "throughput {}/{}: median={:.0} req/s min={:.0} max={:.0} median_aggregate={:.0} ns/request",
            measurement.workload.name(),
            measurement.target.name(),
            measurement.median_requests_per_second(),
            Measurement::minimum(&measurement.requests_per_second),
            Measurement::maximum(&measurement.requests_per_second),
            Measurement::median(&measurement.nanoseconds_per_request),
        )
        .expect("writing throughput measurements to stderr should succeed");
    }
}

fn throughput(c: &mut Criterion) {
    assert_cpu_work_is_deterministic_and_scaled();
    assert_equivalent();

    let mut stderr = std::io::stderr().lock();
    print_environment(&mut stderr);
    print_standalone_runner(&mut stderr);
    drop(stderr);

    let requests = u64::try_from(shape().requests_per_batch()).expect("a batch fits in u64");
    for workload in Workload::ALL {
        let mut group = c.benchmark_group(format!("routerama_throughput/{}", workload.name()));
        group.throughput(Throughput::Elements(requests));
        for target in Target::ALL {
            group.bench_function(target.name(), |b| {
                b.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        elapsed += pool().run_batch(target, workload).elapsed;
                    }
                    elapsed
                });
            });
        }
        group.finish();
    }
}

criterion_group!(benches, throughput);
criterion_main!(benches);
