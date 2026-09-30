// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-bound services retain coherent worker and pool associations.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::num::NonZeroUsize;
use std::thread::{self, ThreadId};

use arty::runtime::{Builtins, ProcessorCount, Runtime, WorkerPoolPolicy};
use arty::task::JoinHandle;
use futures::future::join_all;
use testing_aids::execute_or_terminate_process;
use thread_aware::ThreadAware;

/// Identifies which thread-aware handle a relocation scenario should move.
#[derive(Clone, Copy)]
enum RelocationTarget {
    /// Relocate a detached `TaskScheduler` clone directly.
    Scheduler,
    /// Relocate a `Builtins` clone directly.
    Builtins,
}

/// The thread that ran a piece of work before and after a relocation.
struct BeforeAfter {
    before: ThreadId,
    after: ThreadId,
}

/// Observations gathered while relocating a thread-aware handle to a different worker.
struct RelocationObservation {
    /// The thread executing the scenario's outermost task (the source worker).
    origin: ThreadId,
    /// Thread that ran a blocking task (`spawn_blocking`) before/after relocation.
    blocking_task: BeforeAfter,
    /// Thread that ran an async task (`spawn`) before/after relocation.
    async_task: BeforeAfter,
}

/// Builds a two-processor runtime with the given worker-pool policy, relocates the
/// requested thread-aware handle from the current worker to a different worker, and
/// records the threads that ran a blocking task and an async task before and after.
#[cfg(test)]
fn relocate_and_observe(policy: WorkerPoolPolicy, target: RelocationTarget) -> RelocationObservation {
    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()))
        .worker_pool_policy(policy)
        .build()
        .expect("failed to build runtime");
    let scheduler = runtime.task_scheduler();

    execute_or_terminate_process(move || {
        runtime.run(async move |cx: Builtins| {
            let origin = thread::current().id();
            let here = cx.thread().clone();

            // Discover a different worker by relocating a Builtins clone onto every worker.
            let threads =
                join_all((0..2).map(|_| scheduler.spawn_anywhere(cx.clone(), |worker: Builtins| async move { worker.thread().clone() })))
                    .await;

            let there = threads
                .into_iter()
                .find(|thread| thread != &here)
                .expect("a two-processor runtime must expose a second worker");

            match target {
                RelocationTarget::Scheduler => {
                    let mut scheduler = cx.scheduler().clone();

                    let blocking_before = scheduler.spawn_blocking(|| thread::current().id()).await;
                    let async_before = scheduler.spawn(async move |_| thread::current().id()).await;

                    scheduler.relocate(Some(&here), &there);

                    let blocking_after = scheduler.spawn_blocking(|| thread::current().id()).await;
                    let async_after = scheduler.spawn(async move |_| thread::current().id()).await;

                    RelocationObservation {
                        origin,
                        blocking_task: BeforeAfter {
                            before: blocking_before,
                            after: blocking_after,
                        },
                        async_task: BeforeAfter {
                            before: async_before,
                            after: async_after,
                        },
                    }
                }
                RelocationTarget::Builtins => {
                    let mut builtins = cx.clone();

                    let blocking_before = builtins.scheduler().spawn_blocking(|| thread::current().id()).await;
                    let async_before = builtins.scheduler().spawn(async move |_| thread::current().id()).await;

                    builtins.relocate(Some(&here), &there);

                    let blocking_after = builtins.scheduler().spawn_blocking(|| thread::current().id()).await;
                    let async_after = builtins.scheduler().spawn(async move |_| thread::current().id()).await;

                    RelocationObservation {
                        origin,
                        blocking_task: BeforeAfter {
                            before: blocking_before,
                            after: blocking_after,
                        },
                        async_task: BeforeAfter {
                            before: async_before,
                            after: async_after,
                        },
                    }
                }
            }
        })
    })
}

/// Async tasks always run on the worker the scheduler is currently associated with,
/// regardless of the worker-pool policy: before relocation on the origin worker,
/// after relocation on the (distinct) destination worker.
#[cfg_attr(test, mutants::skip)] // Test oracle, not production behavior; assertion mutations invalidate the test itself.
fn assert_async_follows_relocation(observation: &RelocationObservation) {
    assert_eq!(
        observation.async_task.before, observation.origin,
        "before relocation an async task must run on the origin worker"
    );
    assert_ne!(
        observation.async_task.before, observation.async_task.after,
        "after relocation an async task must run on the destination worker"
    );
}

#[test]
fn relocating_scheduler_uses_destination_pool_when_isolated() {
    let observation = relocate_and_observe(WorkerPoolPolicy::isolated(), RelocationTarget::Scheduler);

    assert_ne!(
        observation.blocking_task.before, observation.blocking_task.after,
        "an isolated pool must dispatch blocking tasks to the destination worker's pool after relocating the scheduler"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_scheduler_uses_same_pool_when_shared() {
    let observation = relocate_and_observe(WorkerPoolPolicy::shared(1), RelocationTarget::Scheduler);

    assert_eq!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a shared single-threaded pool must keep dispatching blocking tasks to the same thread after relocating the scheduler"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_builtins_uses_destination_pool_when_isolated() {
    let observation = relocate_and_observe(WorkerPoolPolicy::isolated(), RelocationTarget::Builtins);

    assert_ne!(
        observation.blocking_task.before, observation.blocking_task.after,
        "an isolated pool must dispatch blocking tasks to the destination worker's pool after relocating Builtins"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_builtins_uses_same_pool_when_shared() {
    let observation = relocate_and_observe(WorkerPoolPolicy::shared(1), RelocationTarget::Builtins);

    assert_eq!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a shared single-threaded pool must keep dispatching blocking tasks to the same thread after relocating Builtins"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn foreign_owner_relocation_preserves_runtime_binding() {
    let source_runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .build()
        .unwrap();
    let destination_runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .build()
        .unwrap();
    let mut builtins = source_runtime.task_scheduler().spawn(async |cx| cx).wait();
    let source = builtins.thread().clone();
    let destination = destination_runtime.task_scheduler().spawn(async |cx| cx.thread().clone()).wait();
    let blocking_thread = builtins.scheduler().spawn_blocking(|| thread::current().id()).wait();

    builtins.relocate(Some(&source), &destination);

    assert_eq!(
        (
            builtins.thread(),
            builtins.scheduler().spawn(async |_| thread::current().id()).wait(),
            builtins.scheduler().spawn_blocking(|| thread::current().id()).wait(),
        ),
        (&source, source.id(), blocking_thread),
    );
}

#[test]
fn repeated_spawn_after_relocation_uses_destination() {
    // Exercise the cached route repeatedly without turning this regression test into a load test.
    const SPAWN_COUNT: usize = 100;

    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()))
        .build()
        .unwrap();
    let workers: Vec<_> = (0..2)
        .map(|_| runtime.task_scheduler().spawn(async |cx| cx))
        .map(JoinHandle::wait)
        .collect();
    let mut scheduler = workers[0].scheduler().clone();
    scheduler.relocate(Some(workers[0].thread()), workers[1].thread());
    scheduler.relocate(None, workers[1].thread());
    scheduler.relocate(Some(workers[1].thread()), workers[1].thread());

    let actual: Vec<_> = (0..SPAWN_COUNT)
        .map(|_| scheduler.spawn(async |_| thread::current().id()))
        .map(JoinHandle::wait)
        .collect();

    assert_eq!(actual, vec![workers[1].thread().id(); SPAWN_COUNT]);
}

#[test]
fn foreign_owner_relocation_preserves_bare_scheduler_binding() {
    let source_runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .build()
        .unwrap();
    let destination_runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .build()
        .unwrap();
    let builtins = source_runtime.task_scheduler().spawn(async |cx| cx).wait();
    let mut scheduler = builtins.scheduler().clone();
    let destination = destination_runtime.task_scheduler().spawn(async |cx| cx.thread().clone()).wait();
    let blocking_thread = scheduler.spawn_blocking(|| thread::current().id()).wait();

    scheduler.relocate(Some(builtins.thread()), &destination);

    assert_eq!(
        (
            scheduler.spawn(async |_| thread::current().id()).wait(),
            scheduler.spawn_blocking(|| thread::current().id()).wait(),
        ),
        (builtins.thread().id(), blocking_thread),
    );
}
