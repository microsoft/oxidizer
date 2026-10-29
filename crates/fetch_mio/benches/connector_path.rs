// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures deterministic DNS and Happy Eyeballs connector paths.

#![allow(missing_docs, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion};
use fetch_mio::benchmark::{Harness, Output, Scenario, run};

const GROUP: &str = "connector_path/connect";

#[metabench::benchmark(ONE_ADDRESS, GROUP, "one_address")]
#[bench::one(Harness::new(Scenario::OneAddress))]
fn one_address(harness: Harness) -> Output {
    run(harness)
}

#[metabench::benchmark(TWO_ADDRESSES_FIRST_SUCCESS, GROUP, "two_addresses_first_success")]
#[bench::two(Harness::new(Scenario::TwoAddressesFirstSuccess))]
fn two_addresses_first_success(harness: Harness) -> Output {
    run(harness)
}

#[metabench::benchmark(DELAYED_SECOND_LAUNCH, GROUP, "delayed_second_launch")]
#[bench::delayed(Harness::new(Scenario::DelayedSecondLaunch))]
fn delayed_second_launch(harness: Harness) -> Output {
    run(harness)
}

#[metabench::benchmark(FAST_FAILURE_THEN_SUCCESS, GROUP, "fast_failure_then_success")]
#[bench::fallback(Harness::new(Scenario::FastFailureThenSuccess))]
fn fast_failure_then_success(harness: Harness) -> Output {
    run(harness)
}

#[metabench::benchmark(ALL_FAILURES, GROUP, "all_failures")]
#[bench::all_failed(Harness::new(Scenario::AllFailures))]
fn all_failures(harness: Harness) -> Output {
    run(harness)
}

const CASES: [(Scenario, &str, &str); 5] = [
    (Scenario::OneAddress, "one_address", "one"),
    (Scenario::TwoAddressesFirstSuccess, "two_addresses_first_success", "two"),
    (Scenario::DelayedSecondLaunch, "delayed_second_launch", "delayed"),
    (Scenario::FastFailureThenSuccess, "fast_failure_then_success", "fallback"),
    (Scenario::AllFailures, "all_failures", "all_failed"),
];

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(GROUP);

    for (scenario, benchmark, case) in CASES {
        group.bench_with_input(BenchmarkId::new(benchmark, case), &scenario, |bencher, scenario| {
            bencher.iter_batched(
                || Harness::new(*scenario),
                |harness| {
                    let output = run(harness);
                    black_box(output.launches());
                    black_box(output.succeeded());
                    output.retain();
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        ONE_ADDRESS,
        TWO_ADDRESSES_FIRST_SUCCESS,
        DELAYED_SECOND_LAUNCH,
        FAST_FAILURE_THEN_SUCCESS,
        ALL_FAILURES,
    ],
    allocator = std::alloc::System,
);
