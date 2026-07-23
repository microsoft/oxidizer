// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Concurrent, CPU-bound throughput for five frameworks and one control.
//!
//! This is an *in-process* throughput measurement: complete dispatch,
//! extraction, an identical deterministic CPU-bound handler, response
//! conversion, and complete body observation, run concurrently on several
//! worker threads. There is no socket, no HTTP parsing, and no framework
//! server, because transport equality is not controllable across these five
//! frameworks; `docs/PERF.md` records that decision and its justification.
//!
//! There is deliberately no paired Callgrind benchmark. An instruction count
//! is a single-threaded, deterministic measure and says nothing about
//! throughput under concurrency; the per-request instruction counts that *are*
//! meaningful are already published by the dispatch, scaling, body, and form
//! fixtures.
//!
//! Running this benchmark prints a fixed-count requests-per-second table to
//! stderr before Criterion starts, so one command produces both the repeated
//! standalone measurement and the Criterion throughput estimate.

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
