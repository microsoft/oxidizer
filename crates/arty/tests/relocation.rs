// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-bound services retain coherent worker and pool associations.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

mod support;

use std::thread::{self, ThreadId};

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
use arty::task::{Builtins, Scheduler};
use futures::future::join_all;
use support::JoinHandleExt as _;
use thread_aware::{ThreadAware, ThreadBuilder};

#[cfg(not(miri))]
#[test]
fn a_foreign_owner_cannot_rebind_a_registered_thread_id() {
    struct RelocationSource(Option<arty::core::Thread>);

    impl ThreadAware for RelocationSource {
        fn relocate(&mut self, source: Option<&arty::core::Thread>, _: &arty::core::Thread) {
            self.0 = source.cloned();
        }
    }

    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let (source, mut scheduler) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (cx.thread().clone(), cx.scheduler().clone()) })
        .join()
        .unwrap();
    let foreign = ThreadBuilder::default().build(source.id());
    assert_ne!(source.owner(), foreign.owner());

    scheduler.relocate(None, &foreign);
    let scheduler: &Scheduler = scheduler.as_ref();
    let actual = scheduler
        .spawn_anywhere(RelocationSource(None), |probe| async move { probe.0 })
        .join()
        .unwrap();
    assert_eq!(actual, Some(source));
}

/// Identifies which thread-aware handle a relocation scenario should move.
#[derive(Clone, Copy)]
enum RelocationTarget {
    /// Relocate a worker-bound `Scheduler` clone directly.
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
fn relocate_and_observe(policy: BlockingPoolPolicy, target: RelocationTarget) -> RelocationObservation {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .blocking_pool(policy)
        .build()
        .expect("failed to build runtime");
    runtime
        .scheduler()
        .block_on(async move |cx: Builtins| {
            let scheduler = cx.scheduler();
            let origin = thread::current().id();
            let here = cx.thread().clone();

            // Discover a different worker by relocating a Builtins clone onto every worker.
            let threads =
                join_all((0..2).map(|_| scheduler.spawn_anywhere(cx.clone(), |worker: Builtins| async move { worker.thread().clone() })))
                    .await;

            let there = threads
                .into_iter()
                .map(Result::unwrap)
                .find(|thread| thread != &here)
                .expect("a two-processor runtime must expose a second worker");

            match target {
                RelocationTarget::Scheduler => {
                    let mut scheduler = cx.scheduler().clone();

                    let blocking_before = scheduler.spawn_blocking(|| thread::current().id()).await.unwrap();
                    let async_before = scheduler.spawn(async move |_| thread::current().id()).await.unwrap();

                    scheduler.relocate(Some(&here), &there);

                    let blocking_after = scheduler.spawn_blocking(|| thread::current().id()).await.unwrap();
                    let async_after = scheduler.spawn(async move |_| thread::current().id()).await.unwrap();

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

                    let blocking_before = builtins.scheduler().spawn_blocking(|| thread::current().id()).await.unwrap();
                    let async_before = builtins.scheduler().spawn(async move |_| thread::current().id()).await.unwrap();

                    builtins.relocate(Some(&here), &there);

                    let blocking_after = builtins.scheduler().spawn_blocking(|| thread::current().id()).await.unwrap();
                    let async_after = builtins.scheduler().spawn(async move |_| thread::current().id()).await.unwrap();

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
        .unwrap()
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
    let observation = relocate_and_observe(BlockingPoolPolicy::isolated(), RelocationTarget::Scheduler);

    assert_ne!(
        observation.blocking_task.before, observation.blocking_task.after,
        "an isolated pool must dispatch blocking tasks to the destination worker's pool after relocating the scheduler"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_scheduler_uses_same_pool_when_shared() {
    let observation = relocate_and_observe(BlockingPoolPolicy::shared(1), RelocationTarget::Scheduler);

    assert_eq!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a shared single-threaded pool must keep dispatching blocking tasks to the same thread after relocating the scheduler"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_builtins_uses_destination_pool_when_isolated() {
    let observation = relocate_and_observe(BlockingPoolPolicy::isolated(), RelocationTarget::Builtins);

    assert_ne!(
        observation.blocking_task.before, observation.blocking_task.after,
        "an isolated pool must dispatch blocking tasks to the destination worker's pool after relocating Builtins"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_builtins_uses_same_pool_when_shared() {
    let observation = relocate_and_observe(BlockingPoolPolicy::shared(1), RelocationTarget::Builtins);

    assert_eq!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a shared single-threaded pool must keep dispatching blocking tasks to the same thread after relocating Builtins"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn foreign_owner_relocation_preserves_runtime_binding() {
    let source_runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let destination_runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let mut builtins = source_runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx })
        .join()
        .unwrap();
    let source = builtins.thread().clone();
    let destination = destination_runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.thread().clone() })
        .join()
        .unwrap();
    let blocking_thread = builtins.scheduler().spawn_blocking(|| thread::current().id()).join().unwrap();

    builtins.relocate(Some(&source), &destination);

    assert_eq!(
        (
            builtins.thread(),
            builtins.scheduler().spawn(async |_| thread::current().id()).join().unwrap(),
            builtins.scheduler().spawn_blocking(|| thread::current().id()).join().unwrap(),
        ),
        (&source, source.id(), blocking_thread),
    );
}

#[test]
fn repeated_spawn_after_relocation_uses_destination() {
    // Exercise the cached route repeatedly without turning this regression test into a load test.
    const SPAWN_COUNT: usize = 100;

    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let workers: Vec<_> = (0..2)
        .map(|_| runtime.scheduler().spawn_anywhere((), |cx, ()| async move { cx }))
        .map(|handle| handle.join().unwrap())
        .collect();
    let mut scheduler = workers[0].scheduler().clone();
    scheduler.relocate(Some(workers[0].thread()), workers[1].thread());
    scheduler.relocate(None, workers[1].thread());
    scheduler.relocate(Some(workers[1].thread()), workers[1].thread());

    let actual: Vec<_> = (0..SPAWN_COUNT)
        .map(|_| scheduler.spawn(async |_| thread::current().id()))
        .map(|handle| handle.join().unwrap())
        .collect();

    assert_eq!(actual, vec![workers[1].thread().id(); SPAWN_COUNT]);
}

#[test]
fn foreign_owner_relocation_preserves_bare_scheduler_binding() {
    let source_runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let destination_runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let builtins = source_runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx })
        .join()
        .unwrap();
    let mut scheduler = builtins.scheduler().clone();
    let destination = destination_runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.thread().clone() })
        .join()
        .unwrap();
    let blocking_thread = scheduler.spawn_blocking(|| thread::current().id()).join().unwrap();

    scheduler.relocate(Some(builtins.thread()), &destination);

    assert_eq!(
        (
            scheduler.spawn(async |_| thread::current().id()).join().unwrap(),
            scheduler.spawn_blocking(|| thread::current().id()).join().unwrap(),
        ),
        (builtins.thread().id(), blocking_thread),
    );
}
