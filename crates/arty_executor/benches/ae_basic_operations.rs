// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for basic operations of the executor.

use std::pin::pin;
use std::task::{Context, Waker};
use std::time::Instant;

use alloc_tracker::Allocator;
use arty_executor::CycleOutcome;
use arty_executor::testing::new_guarded_executor;
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use testing_aids::YieldFuture;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();
    let time = all_the_time::Session::new();

    let mut group = c.benchmark_group("ae_basic_operations/basic");

    bench_spawn_and_complete_one(&mut group, &allocs, &time);
    bench_yield_one(&mut group, &allocs, &time);
    bench_noop(&mut group, &allocs, &time);

    group.finish();

    let mut group = c.benchmark_group("ae_basic_operations/slow");

    bench_spawn_and_complete_one_times_many(&mut group, &allocs, &time);
    bench_spawn_and_complete_10k(&mut group, &allocs, &time);
    bench_yield_10k(&mut group, &allocs, &time);

    group.finish();
}

fn bench_spawn_and_complete_one(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const NAME: &str = "spawn_and_complete_one";

    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();

    let body: Box<dyn Fn()> = Box::new({
        move || {
            // We spawn a task.
            let mut join_handle = pin!(tasks.add(async move {}));

            // And we wait for it to complete. We use forbidden white-box knowledge
            // that it only takes one cycle for an empty task to complete.
            // While executing executor cycles (really, one should be enough).
            assert_ne!(executor.execute_cycle(), CycleOutcome::Shutdown);

            let mut cx = Context::from_waker(Waker::noop());
            let result = join_handle.as_mut().poll(&mut cx);

            assert!(result.is_ready());
        }
    });

    // Execute it once to preallocate data sets when not measuring memory allocations.
    body();

    let allocs_op = allocs.operation(NAME);
    let time_op = time.operation(NAME);
    group.bench_function(NAME, |b| {
        b.iter_custom(|iters| {
            let _allocs_span = allocs_op.measure_thread().iterations(iters);
            let _time_span = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}

fn bench_spawn_and_complete_one_times_many(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    const NAME: &str = "spawn_and_complete_one_times_many";
    const MANY: usize = 1000;

    // This is the same as the `spawn_and_complete_one` benchmark, but we run it MANY times.
    // The idea is to examine whether there is some "accumulation" of overhead - is it more
    // expensive than MANY x the baseline? That may hint at "garbage collecting up" somewhere.
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();

    let body: Box<dyn Fn()> = Box::new({
        move || {
            for _ in 0..MANY {
                // We spawn a task.
                let mut join_handle = pin!(tasks.add(async move {}));

                // And we wait for it to complete. We use forbidden white-box knowledge
                // that it only takes one cycle for an empty task to complete.
                // While executing executor cycles (really, one should be enough).
                assert_ne!(executor.execute_cycle(), CycleOutcome::Shutdown);

                let mut cx = Context::from_waker(Waker::noop());
                let result = join_handle.as_mut().poll(&mut cx);

                assert!(result.is_ready());
            }
        }
    });

    // Execute it once to preallocate data sets when not measuring memory allocations.
    body();

    let allocs_op = allocs.operation(NAME);
    let time_op = time.operation(NAME);
    group.bench_function(NAME, |b| {
        b.iter_custom(|iters| {
            let _allocs_span = allocs_op.measure_thread().iterations(iters);
            let _time_span = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}

fn bench_spawn_and_complete_10k(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const NAME: &str = "spawn_and_complete_10k";

    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();

    let mut join_handles = Vec::with_capacity(10_000);

    let mut body: Box<dyn FnMut()> = Box::new({
        move || {
            join_handles.clear();

            // We spawn tasks.
            for _ in 0..10_000 {
                join_handles.push(tasks.add(async move {}));
            }

            // And we wait for it to complete. We use forbidden white-box knowledge
            // that it only takes one cycle for an empty task to complete.
            // While executing executor cycles (really, one should be enough).
            assert_ne!(executor.execute_cycle(), CycleOutcome::Shutdown);

            let mut cx = Context::from_waker(Waker::noop());

            for join_handle in &mut join_handles {
                let mut join_handle = pin!(join_handle);
                let result = join_handle.as_mut().poll(&mut cx);
                assert!(result.is_ready());
            }
        }
    });

    // Execute it once to preallocate data sets when not measuring memory allocations.
    body();

    let allocs_op = allocs.operation(NAME);
    let time_op = time.operation(NAME);
    group.bench_function(NAME, |b| {
        b.iter_custom(|iters| {
            let _allocs_span = allocs_op.measure_thread().iterations(iters);
            let _time_span = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}

fn bench_yield_one(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const NAME: &str = "yield_one";

    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();

    let body: Box<dyn Fn()> = Box::new({
        move || {
            // We spawn a yield task. This will suspend once, self-awaken, then complete.
            let mut join_handle = pin!(tasks.add(YieldFuture::default()));

            // The self-awakening will request an immediate continuation.
            assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

            // And we wait for it to complete. We use forbidden white-box knowledge
            // that it only takes one cycle for the yield task to complete.
            assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

            let mut cx = Context::from_waker(Waker::noop());
            let result = join_handle.as_mut().poll(&mut cx);

            assert!(result.is_ready());
        }
    });

    // Execute it once to preallocate data sets when not measuring memory allocations.
    body();

    let allocs_op = allocs.operation(NAME);
    let time_op = time.operation(NAME);
    group.bench_function(NAME, |b| {
        b.iter_custom(|iters| {
            // Note: we measure these also for the "prepare" phase because essentially we have no
            // choice - we must reuse the executor and this means we must prepare and measure one
            // by one, which is not compatible with at least processor time measurements due to
            // high overhead. Same goes for wall clock time, really.
            let _allocs_span = allocs_op.measure_thread().iterations(iters);
            let _time_span = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}

fn bench_yield_10k(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const NAME: &str = "yield_10k";

    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();

    // Reusing the join handle buffer to avoid allocating memory in hot loop.
    let mut join_handles = Vec::with_capacity(10_000);

    let mut body: Box<dyn FnMut()> = Box::new({
        move || {
            join_handles.clear();

            for _ in 0..10_000 {
                join_handles.push(tasks.add(YieldFuture::default()));
            }

            // The self-awakening will request an immediate continuation.
            assert_eq!(executor.execute_cycle(), CycleOutcome::Continue);

            // And we wait for it to complete. We use forbidden white-box knowledge
            // that it only takes one cycle for the yield task to complete.
            assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

            let mut cx = Context::from_waker(Waker::noop());

            for join_handle in &mut join_handles {
                let mut join_handle = pin!(join_handle);
                let result = join_handle.as_mut().poll(&mut cx);
                assert!(result.is_ready());
            }
        }
    });

    // Execute it once to preallocate data sets when not measuring memory allocations.
    body();

    let allocs_op = allocs.operation(NAME);
    let time_op = time.operation(NAME);
    group.bench_function(NAME, |b| {
        b.iter_custom(|iters| {
            // Note: we measure these also for the "prepare" phase because essentially we have no
            // choice - we must reuse the executor and this means we must prepare and measure one
            // by one, which is not compatible with at least processor time measurements due to
            // high overhead. Same goes for wall clock time, really.
            let _allocs_span = allocs_op.measure_thread().iterations(iters);
            let _time_span = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}

fn bench_noop(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const NAME: &str = "noop";

    let executor = new_guarded_executor(Waker::noop().clone());

    let body: Box<dyn Fn()> = Box::new({
        move || {
            // The executor has nothing to do and it does nothing, just asks us to go away.
            assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
        }
    });

    // Execute it once to preallocate data sets when not measuring memory allocations.
    body();

    let allocs_op = allocs.operation(NAME);
    let time_op = time.operation(NAME);
    group.bench_function(NAME, |b| {
        b.iter_custom(|iters| {
            let _allocs_span = allocs_op.measure_thread().iterations(iters);
            let _time_span = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}
