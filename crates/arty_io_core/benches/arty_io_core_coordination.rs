// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coordinator cycle overhead for the common primary-only and primary-plus-secondary shapes.
//!
//! The workloads model already-armed native waits. They measure coordination and callback
//! overhead, not operating-system waiting or cross-thread scheduling.

use std::hint::black_box;
use std::task::Waker;
use std::time::{Duration, Instant};

use arty_io_core::{Coordinator, Cycle};
use criterion::{BatchSize, Criterion};

const CYCLE_GROUP: &str = "arty_io_core_coordination/cycle";

struct Input {
    coordinator: Coordinator,
    started_at: Instant,
}

impl Input {
    fn prepared(workload: fn(&mut Self)) -> Self {
        let mut input = Self {
            coordinator: Coordinator::new(),
            started_at: Instant::now(),
        };
        workload(&mut input);
        input.started_at = Instant::now();
        input
    }
}

fn single_primary_cycle(input: &mut Input) {
    input.coordinator.begin_cycle();
    let cycle = Cycle::new(black_box(input.started_at), black_box(Duration::ZERO), &input.coordinator);
    let mut primary = cycle.start_work();
    primary.on_interrupt(Waker::noop().clone());
    primary.complete();
    input.coordinator.complete_cycle();
}

fn primary_one_secondary_cycle(input: &mut Input) {
    input.coordinator.begin_cycle();
    let cycle = Cycle::new(black_box(input.started_at), black_box(Duration::ZERO), &input.coordinator);
    let mut secondary = cycle.start_work();
    secondary.on_interrupt(Waker::noop().clone());
    let mut primary = cycle.start_work();
    primary.on_interrupt(Waker::noop().clone());
    secondary.complete();
    primary.complete();
    input.coordinator.complete_cycle();
}

#[metabench::benchmark(SINGLE_PRIMARY, CYCLE_GROUP, "single_primary")]
#[bench::default(Input::prepared(single_primary_cycle))]
fn single_primary(mut input: Input) -> Input {
    single_primary_cycle(&mut input);
    input
}

#[metabench::benchmark(PRIMARY_ONE_SECONDARY, CYCLE_GROUP, "primary_one_secondary")]
#[bench::default(Input::prepared(primary_one_secondary_cycle))]
fn primary_one_secondary(mut input: Input) -> Input {
    primary_one_secondary_cycle(&mut input);
    input
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(CYCLE_GROUP);
    group.bench_function(SINGLE_PRIMARY.benchmark_name(), |bencher| {
        bencher.iter_batched(|| Input::prepared(single_primary_cycle), single_primary, BatchSize::SmallInput);
    });
    group.bench_function(PRIMARY_ONE_SECONDARY.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || Input::prepared(primary_one_secondary_cycle),
            primary_one_secondary,
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [SINGLE_PRIMARY, PRIMARY_ONE_SECONDARY],
);
