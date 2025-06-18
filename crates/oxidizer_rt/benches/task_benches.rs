// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::arithmetic_side_effects,
    reason = "it is fine to let our guard down in benchmark/test code"
)]

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll::{Pending, Ready};
use std::thread;
use std::time::{Duration, Instant};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main};
use oxidizer_rt::{BasicThreadState, Runtime};

fn criterion_benchmark(c: &mut Criterion) {
    group_spawn(c);
    group_spawn_local(c);
    group_pending(c);
}

/// Benchmark completing a pending future.
///
/// We measure the time it takes to complete `count` number of tasks that await a future that is pending.
/// This is done to measure the overhead of the runtime when the future is not ready.
fn group_pending(c: &mut Criterion) {
    let mut group = c.benchmark_group("pending");
    for count in &[1, 3, 5, 7] {
        group.throughput(criterion::Throughput::Elements(*count));

        oxidizer_pending(&mut group, *count);
        tokio_pending(&mut group, *count);
    }
    group.finish();
}

/// Measure the time it takes to complete a pending future with tokio.
fn tokio_pending(group: &mut BenchmarkGroup<'_, WallTime>, count: u64) {
    group.bench_with_input(BenchmarkId::new("Tokio", count), &count, |b, count| {
        let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();

        b.iter(|| {
            let handles = (0..*count)
                .map(|_| {
                    let future = PendingFuture::new();
                    runtime.spawn(async move {
                        future.await;
                    })
                })
                .collect::<Vec<_>>();

            for handle in handles {
                let _ = runtime.block_on(handle);
            }
        });

        runtime.shutdown_timeout(Duration::from_millis(10));
    });
}

/// Measure the time it takes to complete a pending future with oxidizer.
///
/// `PendingFuture` is a future that is pending, then immediately wakes up the waker, using a thread, and completes.
fn oxidizer_pending(group: &mut BenchmarkGroup<'_, WallTime>, count: u64) {
    group.bench_with_input(BenchmarkId::new("Oxidizer", count), &count, |b, count| {
        let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

        b.iter(|| {
            let handles = (0..*count)
                .map(|_| {
                    let future = PendingFuture::new();
                    runtime.spawn(async move |_cx| {
                        future.await;
                    })
                })
                .collect::<Vec<_>>();

            for mut handle in handles {
                handle.wait();
            }
        });
    });
}

/// Benchmark spawning a thread-local task.
fn group_spawn_local(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn local");

    for count in &[1, 2, 10, 100, 1000] {
        group.throughput(criterion::Throughput::Elements(*count));

        oxidizer_spawn_local(&mut group, *count);
        tokio_spawn_local(&mut group, *count);
    }
    group.finish();
}

/// Benchmark spawning a thread-local task with tokio.
///
/// The runtime start time is not taking into account.
/// We measure the time it takes to complete `count` number of thread-local tasks.
/// We use `LocalSet` to spawn the tasks.
fn tokio_spawn_local(group: &mut BenchmarkGroup<'_, WallTime>, count: u64) {
    group.bench_with_input(BenchmarkId::new("Tokio", count), &count, |b, count| {
        let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();
        let guard = runtime.enter();
        let count = *count;

        b.iter(|| {
            runtime.block_on(async move {
                let set = tokio::task::LocalSet::new();
                set.run_until(async move {
                    let handles = (0..count)
                        .map(|_| tokio::task::spawn_local(async move { black_box(()) }))
                        .collect::<Vec<_>>();

                    for handle in handles {
                        handle.await.unwrap();
                    }
                })
                .await;
            });
        });

        drop(guard);
        runtime.shutdown_timeout(Duration::from_millis(10));
    });
}

/// Benchmark spawning a thread-local task with oxidizer.
///
/// The runtime start time is not taking into account.
/// We only measure the time `spawn_local` takes to spawn `count` number of tasks and complete them.
/// Although we do spawn a task to get access to `spawn_local` that time is not accounted for.
fn oxidizer_spawn_local(group: &mut BenchmarkGroup<'_, WallTime>, count: u64) {
    group.bench_with_input(BenchmarkId::new("Oxidizer", count), &count, |b, count| {
        let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");
        let count = *count;

        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;

            for _ in 0..iters {
                total += runtime
                    .spawn(async move |cx| {
                        let now = Instant::now();

                        let handles = (0..count)
                            .map(|_| cx.local_scheduler().spawn(async move || black_box(())))
                            .collect::<Vec<_>>();

                        for handle in handles {
                            handle.await;
                        }

                        now.elapsed()
                    })
                    .wait();
            }

            total
        });
    });
}

/// Benchmark spawning a task.
fn group_spawn(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn");

    // We measure several different counts of tasks to spawn to see how the runtimes scale.
    for count in &[1, 2, 10, 100, 1000] {
        group.throughput(criterion::Throughput::Elements(*count));

        oxidizer_spawn(&mut group, *count);
        tokio_spawn(&mut group, *count);
    }

    group.finish();
}

/// Benchmark spawning a task with tokio.
/// The runtime start time is not taking into account.
///
/// We measure spawning `count` number of tasks and wait for them to complete.
fn tokio_spawn(group: &mut BenchmarkGroup<'_, WallTime>, count: u64) {
    group.bench_with_input(BenchmarkId::new("Tokio", count), &count, |b, count| {
        let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();

        b.iter(|| {
            let handles = (0..*count)
                .map(|_| runtime.spawn(async move { black_box(()) }))
                .collect::<Vec<_>>();

            for handle in handles {
                let _ = runtime.block_on(handle);
            }
        });

        runtime.shutdown_timeout(Duration::from_millis(10));
    });
}

/// Benchmark spawning a task with oxidizer.
/// The runtime start time is not taking into account.
///
/// We measure spawning `count` number of tasks and wait for them to complete.
fn oxidizer_spawn(group: &mut BenchmarkGroup<'_, WallTime>, count: u64) {
    group.bench_with_input(BenchmarkId::new("Oxidizer", count), &count, |b, count| {
        let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

        b.iter(|| {
            let handles = (0..*count)
                .map(|_| runtime.spawn(async move |_cx| black_box(())))
                .collect::<Vec<_>>();

            for mut handle in handles {
                handle.wait();
            }
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = criterion_benchmark
}

criterion_main!(benches);

struct PendingFuture(AtomicBool);

impl PendingFuture {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
}

impl Future for PendingFuture {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let waker = cx.waker().clone();
        thread::spawn(move || {
            waker.wake();
        });
        if self.0.load(Ordering::Relaxed) {
            Ready(())
        } else {
            self.0.store(true, Ordering::Relaxed);
            Pending
        }
    }
}