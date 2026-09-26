// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coordinator cycles and individual coordination operations.
//!
//! The workloads model already-armed native waits. They measure coordination and callback
//! overhead, not operating-system waiting or cross-thread scheduling.

use std::hint::black_box;
use std::task::Waker;
use std::time::{Duration, Instant};

use arty_io_core::{Coordinator, Cycle, PendingWork};
use criterion::{BatchSize, BenchmarkId, Criterion};

const CYCLE_GROUP: &str = "arty_io_core_coordination/cycle";
const OPERATION_GROUP: &str = "arty_io_core_coordination/operation";

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

fn idle_cycle(input: &mut Input) {
    input.coordinator.begin_cycle();
    input.coordinator.complete_cycle();
}

fn unpublished_work_cycle(input: &mut Input) {
    input.coordinator.begin_cycle();
    let cycle = Cycle::new(black_box(input.started_at), black_box(Duration::ZERO), &input.coordinator);
    drop(cycle.start_work());
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

#[metabench::benchmark(IDLE, CYCLE_GROUP, "idle")]
#[bench::default(Input::prepared(idle_cycle))]
fn idle(mut input: Input) -> Input {
    idle_cycle(&mut input);
    input
}

#[metabench::benchmark(UNPUBLISHED_WORK, CYCLE_GROUP, "unpublished_work")]
#[bench::default(Input::prepared(unpublished_work_cycle))]
fn unpublished_work(mut input: Input) -> Input {
    unpublished_work_cycle(&mut input);
    input
}

struct OperationInput {
    coordinator: Coordinator,
    started_at: Instant,
    work: Option<PendingWork>,
    interrupt: Waker,
}

impl OperationInput {
    fn prepared(registrations: usize, interrupted: bool) -> Self {
        let mut coordinator = Coordinator::new();
        coordinator.begin_cycle();
        let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
        let mut work = cycle.start_work();
        for _ in 0..registrations {
            work.on_interrupt(black_box(Waker::noop().clone()));
        }
        let interrupt = coordinator.interrupt_waker();
        if interrupted {
            interrupt.wake_by_ref();
        }
        Self {
            coordinator,
            started_at: Instant::now(),
            work: Some(work),
            interrupt,
        }
    }

    fn idle(interrupted: bool) -> Self {
        let mut input = Self::prepared(0, interrupted);
        input.work.take();
        input
    }

    fn retired(registrations: usize) -> Self {
        let mut input = Self::prepared(registrations, false);
        input.work.take();
        input
    }
}

#[metabench::benchmark(BEGIN_CYCLE, OPERATION_GROUP, "begin_cycle")]
#[bench::idle(OperationInput::idle(false))]
#[bench::completed(OperationInput::idle(true))]
fn begin_cycle(mut input: OperationInput) -> OperationInput {
    input.coordinator.begin_cycle();
    input
}

#[metabench::benchmark(COMPLETE_CYCLE, OPERATION_GROUP, "complete_cycle")]
#[bench::idle(OperationInput::idle(false))]
#[bench::completed(OperationInput::idle(true))]
fn complete_cycle(mut input: OperationInput) -> OperationInput {
    input.coordinator.complete_cycle();
    input
}

#[metabench::benchmark(START_WORK, OPERATION_GROUP, "start_work")]
#[bench::default(OperationInput::idle(false))]
fn start_work(mut input: OperationInput) -> OperationInput {
    let cycle = Cycle::new(black_box(input.started_at), Duration::ZERO, &input.coordinator);
    input.work = Some(cycle.start_work());
    input
}

#[metabench::benchmark(DROP_WORK, OPERATION_GROUP, "drop_work")]
#[bench::default(OperationInput::prepared(0, false))]
#[bench::registered(OperationInput::prepared(1, false))]
#[bench::eight(OperationInput::prepared(8, false))]
#[bench::thirty_two(OperationInput::prepared(32, false))]
fn drop_work(mut input: OperationInput) -> OperationInput {
    drop(input.work.take());
    input
}

#[metabench::benchmark(COMPLETE_WORK, OPERATION_GROUP, "complete_work")]
#[bench::unregistered(OperationInput::prepared(0, false))]
#[bench::registered(OperationInput::prepared(1, false))]
#[bench::interrupted(OperationInput::prepared(1, true))]
fn complete_work(mut input: OperationInput) -> OperationInput {
    input.work.take().expect("setup creates pending work for this operation").complete();
    input
}

#[metabench::benchmark(ON_INTERRUPT, OPERATION_GROUP, "on_interrupt")]
#[bench::first(OperationInput::prepared(0, false))]
#[bench::spare_capacity(OperationInput::prepared(1, false))]
#[bench::interrupted(OperationInput::prepared(1, true))]
fn on_interrupt(mut input: OperationInput) -> OperationInput {
    input
        .work
        .as_mut()
        .expect("setup creates pending work for this operation")
        .on_interrupt(black_box(Waker::noop().clone()));
    input
}

#[metabench::benchmark(IS_INTERRUPTED, OPERATION_GROUP, "is_interrupted")]
#[bench::no(OperationInput::prepared(0, false))]
#[bench::yes(OperationInput::prepared(0, true))]
fn is_interrupted(input: OperationInput) -> OperationInput {
    black_box(
        input
            .work
            .as_ref()
            .expect("setup creates pending work for this operation")
            .is_interrupted(),
    );
    input
}

#[metabench::benchmark(WAKE_BY_REF, OPERATION_GROUP, "wake_by_ref")]
#[bench::empty(OperationInput::prepared(0, false))]
#[bench::one(OperationInput::prepared(1, false))]
#[bench::eight(OperationInput::prepared(8, false))]
#[bench::thirty_two(OperationInput::prepared(32, false))]
#[bench::interrupted(OperationInput::prepared(1, true))]
#[bench::retired(OperationInput::retired(32))]
fn wake_by_ref(input: OperationInput) -> OperationInput {
    input.interrupt.wake_by_ref();
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
    group.bench_function(IDLE.benchmark_name(), |bencher| {
        bencher.iter_batched(|| Input::prepared(idle_cycle), idle, BatchSize::SmallInput);
    });
    group.bench_function(UNPUBLISHED_WORK.benchmark_name(), |bencher| {
        bencher.iter_batched(|| Input::prepared(unpublished_work_cycle), unpublished_work, BatchSize::SmallInput);
    });
    group.finish();

    let mut group = criterion.benchmark_group(OPERATION_GROUP);
    for (case, interrupted) in [("idle", false), ("completed", true)] {
        group.bench_with_input(
            BenchmarkId::new(BEGIN_CYCLE.benchmark_name(), case),
            &interrupted,
            |bencher, &value| {
                bencher.iter_batched(|| OperationInput::idle(value), begin_cycle, BatchSize::SmallInput);
            },
        );
        group.bench_with_input(
            BenchmarkId::new(COMPLETE_CYCLE.benchmark_name(), case),
            &interrupted,
            |bencher, &value| {
                bencher.iter_batched(|| OperationInput::idle(value), complete_cycle, BatchSize::SmallInput);
            },
        );
    }
    group.bench_function(START_WORK.benchmark_name(), |bencher| {
        bencher.iter_batched(|| OperationInput::idle(false), start_work, BatchSize::SmallInput);
    });
    group.bench_function(DROP_WORK.benchmark_name(), |bencher| {
        bencher.iter_batched(|| OperationInput::prepared(0, false), drop_work, BatchSize::SmallInput);
    });
    for (case, registrations) in [("registered", 1), ("eight", 8), ("thirty_two", 32)] {
        group.bench_with_input(
            BenchmarkId::new(DROP_WORK.benchmark_name(), case),
            &registrations,
            |bencher, &count| {
                bencher.iter_batched(|| OperationInput::prepared(count, false), drop_work, BatchSize::SmallInput);
            },
        );
    }
    for (case, registrations, interrupted) in [("unregistered", 0, false), ("registered", 1, false), ("interrupted", 1, true)] {
        group.bench_with_input(
            BenchmarkId::new(COMPLETE_WORK.benchmark_name(), case),
            &(registrations, interrupted),
            |bencher, &(count, raised)| {
                bencher.iter_batched(|| OperationInput::prepared(count, raised), complete_work, BatchSize::SmallInput);
            },
        );
    }
    for (case, registrations, interrupted) in [("first", 0, false), ("spare_capacity", 1, false), ("interrupted", 1, true)] {
        group.bench_with_input(
            BenchmarkId::new(ON_INTERRUPT.benchmark_name(), case),
            &(registrations, interrupted),
            |bencher, &(count, raised)| {
                bencher.iter_batched(|| OperationInput::prepared(count, raised), on_interrupt, BatchSize::SmallInput);
            },
        );
    }
    for (case, interrupted) in [("no", false), ("yes", true)] {
        group.bench_with_input(
            BenchmarkId::new(IS_INTERRUPTED.benchmark_name(), case),
            &interrupted,
            |bencher, &raised| {
                bencher.iter_batched(|| OperationInput::prepared(0, raised), is_interrupted, BatchSize::SmallInput);
            },
        );
    }
    for (case, registrations, interrupted) in [
        ("empty", 0, false),
        ("one", 1, false),
        ("eight", 8, false),
        ("thirty_two", 32, false),
        ("interrupted", 1, true),
    ] {
        group.bench_with_input(
            BenchmarkId::new(WAKE_BY_REF.benchmark_name(), case),
            &(registrations, interrupted),
            |bencher, &(count, raised)| {
                bencher.iter_batched(|| OperationInput::prepared(count, raised), wake_by_ref, BatchSize::SmallInput);
            },
        );
    }
    group.bench_function(BenchmarkId::new(WAKE_BY_REF.benchmark_name(), "retired"), |bencher| {
        bencher.iter_batched(|| OperationInput::retired(32), wake_by_ref, BatchSize::SmallInput);
    });
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        SINGLE_PRIMARY,
        PRIMARY_ONE_SECONDARY,
        IDLE,
        UNPUBLISHED_WORK,
        BEGIN_CYCLE,
        COMPLETE_CYCLE,
        START_WORK,
        DROP_WORK,
        COMPLETE_WORK,
        ON_INTERRUPT,
        IS_INTERRUPTED,
        WAKE_BY_REF
    ],
);
