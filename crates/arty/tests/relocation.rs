// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-bound services retain coherent worker and pool associations.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

mod support;

use std::sync::{Arc, Mutex, PoisonError};
use std::task::Waker;
use std::thread::{self, ThreadId};

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
use arty::task::{Builtins, Scheduler};
use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
use futures::future::join_all;
use support::JoinHandleExt as _;
use thread_aware::{ThreadAware, ThreadBuilder};

#[derive(Clone)]
struct RelocationIoOptions(Arc<Mutex<Vec<ThreadId>>>);

impl ThreadAware for RelocationIoOptions {
    fn relocate(&mut self, _source: Option<&arty::core::Thread>, _destination: &arty::core::Thread) {}
}

#[derive(Clone)]
struct RelocationIoContext(RelocationIoOptions);

impl ThreadAware for RelocationIoContext {
    fn relocate(&mut self, _source: Option<&arty::core::Thread>, _destination: &arty::core::Thread) {}
}

impl IoContext for RelocationIoContext {
    type ContextOptions = RelocationIoOptions;
    type Driver = RelocationIoDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self(options)
    }

    fn driver(&mut self, options: DriverOptions) -> Result<Self::Driver, DriverError> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner).push(options.thread().id());
        _ = options.role();
        Ok(RelocationIoDriver)
    }
}

struct RelocationIoDriver;

impl Driver for RelocationIoDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        Ok(())
    }
}

#[test]
fn relocating_builtins_leaves_shared_source_clones_on_the_source_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let (source, original, scheduler) = runtime
        .scheduler()
        .spawn_anywhere(
            (),
            |cx, ()| async move { (cx.thread().clone(), cx.clone(), cx.scheduler().clone()) },
        )
        .join()
        .unwrap();
    let (destination, relocated_scheduler) = runtime
        .scheduler()
        .spawn_anywhere(original.clone(), |_, relocated| async move {
            (relocated.thread().clone(), relocated.scheduler().clone())
        })
        .join()
        .unwrap();
    assert_ne!(source, destination);
    let original_thread = scheduler
        .spawn(move |_| async move {
            let original_thread = original.thread().clone();
            assert_eq!(original_thread.id(), thread::current().id());
            assert_eq!(
                original.scheduler().spawn(async |_| thread::current().id()).await.unwrap(),
                original_thread.id()
            );
            original_thread
        })
        .join()
        .unwrap();
    assert_eq!(original_thread, source);
    assert_eq!(
        relocated_scheduler.spawn(async |_| thread::current().id()).join().unwrap(),
        destination.id()
    );
    runtime.stop().unwrap();
}

struct ReturnedProbe {
    thread: arty::core::Thread,
    relocations: Vec<(Option<arty::core::Thread>, arty::core::Thread, ThreadId)>,
}

impl ThreadAware for ReturnedProbe {
    fn relocate(&mut self, source: Option<&arty::core::Thread>, destination: &arty::core::Thread) {
        self.thread = destination.clone();
        self.relocations
            .push((source.cloned(), destination.clone(), thread::current().id()));
    }
}

#[test]
fn spawn_anywhere_relocates_results_on_the_source_worker() {
    for (runtime, workers) in [
        (Runtime::new().unwrap(), 1),
        (Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap(), 2),
    ] {
        runtime
            .scheduler()
            .block_on(async move |cx| {
                let source = cx.thread().clone();
                let tasks = (0..workers).map(|_| {
                    cx.scheduler().spawn_anywhere(
                        ReturnedProbe {
                            thread: source.clone(),
                            relocations: Vec::new(),
                        },
                        |probe| async move {
                            testing_aids::YieldFuture::default().await;
                            probe
                        },
                    )
                });
                let mut executed_elsewhere = false;
                for result in join_all(tasks).await {
                    let result = result.unwrap();
                    assert_eq!(result.thread, source);
                    assert_eq!(result.relocations.len(), 2);
                    let (input_source, executed, input_callback) = &result.relocations[0];
                    let (output_source, returned, output_callback) = &result.relocations[1];
                    assert_eq!(input_source.as_ref(), Some(&source));
                    assert_eq!(*input_callback, executed.id());
                    assert_eq!(output_source.as_ref(), Some(executed));
                    assert_eq!(returned, &source);
                    assert_eq!(*output_callback, source.id());
                    executed_elsewhere |= executed != &source;
                }
                assert_eq!(executed_elsewhere, workers > 1);
            })
            .unwrap();
        runtime.stop().unwrap();
    }
}

#[test]
fn spawn_at_relocates_results_to_the_source_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let source = runtime.scheduler().block_on(async |cx| cx).unwrap();
    let destination = source
        .control()
        .workers()
        .into_iter()
        .find(|worker| worker != &source.thread())
        .cloned()
        .unwrap();
    let task = source.scheduler().spawn_at(
        &destination,
        ReturnedProbe {
            thread: source.thread().clone(),
            relocations: Vec::new(),
        },
        |probe| async move { probe },
    );

    source
        .scheduler()
        .spawn(async move |cx| {
            let result = task.await.unwrap();
            assert_eq!(&result.thread, cx.thread());
            assert_eq!(result.relocations.len(), 2);
            let (input_source, executed, input_callback) = &result.relocations[0];
            let (output_source, returned, output_callback) = &result.relocations[1];
            assert_eq!(input_source.as_ref(), Some(cx.thread()));
            assert_eq!(executed, &destination);
            assert_eq!(*input_callback, destination.id());
            assert_eq!(output_source.as_ref(), Some(&destination));
            assert_eq!(returned, cx.thread());
            assert_eq!(*output_callback, cx.thread().id());
        })
        .join()
        .unwrap();
    runtime.stop().unwrap();
}

#[test]
fn spawn_anywhere_returns_builtins_bound_to_the_source_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    runtime
        .scheduler()
        .block_on(async |cx| {
            let source = cx.thread().clone();
            for task in (0..2).map(|_| cx.scheduler().spawn_anywhere(cx.clone(), |moved| async move { moved })) {
                let returned = task.await.unwrap();
                assert_eq!(returned.thread(), &source);
                assert_eq!(
                    returned.scheduler().spawn(async |child| child.thread().id()).await.unwrap(),
                    source.id()
                );
            }
        })
        .unwrap();
    runtime.stop().unwrap();
}

#[test]
fn spawn_anywhere_relocates_results_to_the_worker_that_receives_the_join() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let source = runtime.scheduler().block_on(async |cx| cx).unwrap();
    let destination = runtime.scheduler().block_on(async |cx| cx).unwrap();
    assert_ne!(source.thread(), destination.thread());
    let task = source.scheduler().spawn_anywhere(
        ReturnedProbe {
            thread: source.thread().clone(),
            relocations: Vec::new(),
        },
        |probe| async move { probe },
    );
    destination
        .scheduler()
        .spawn(async move |cx| {
            let result = task.await.unwrap();
            assert_eq!(&result.thread, cx.thread());
            assert_eq!(result.relocations.len(), 2);
            let (_, returned, callback) = &result.relocations[1];
            assert_eq!(returned, cx.thread());
            assert_eq!(*callback, thread::current().id());
        })
        .join()
        .unwrap();
    runtime.stop().unwrap();
}

#[test]
fn spawn_anywhere_results_joined_outside_arty_are_not_relocated() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let source = runtime.scheduler().block_on(async |cx| cx.thread().clone()).unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let result = scheduler
        .spawn_anywhere(
            ReturnedProbe {
                thread: source.clone(),
                relocations: Vec::new(),
            },
            |probe| async move { probe },
        )
        .join()
        .unwrap();
    assert_eq!(result.thread, source);
    assert_eq!(result.relocations.len(), 1);
    assert_eq!(result.relocations[0].2, source.id());
    runtime.stop().unwrap();
}

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
fn relocating_scheduler_uses_destination_pool_when_per_worker() {
    let observation = relocate_and_observe(BlockingPoolPolicy::per_worker(), RelocationTarget::Scheduler);

    assert_ne!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a per-worker pool must dispatch blocking tasks to the destination worker's pool after relocating the scheduler"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_scheduler_uses_same_pool_when_shared() {
    let observation = relocate_and_observe(BlockingPoolPolicy::shared().max(1), RelocationTarget::Scheduler);

    assert_eq!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a shared single-threaded pool must keep dispatching blocking tasks to the same thread after relocating the scheduler"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_builtins_uses_destination_pool_when_per_worker() {
    let observation = relocate_and_observe(BlockingPoolPolicy::per_worker(), RelocationTarget::Builtins);

    assert_ne!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a per-worker pool must dispatch blocking tasks to the destination worker's pool after relocating Builtins"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocating_builtins_uses_same_pool_when_shared() {
    let observation = relocate_and_observe(BlockingPoolPolicy::shared().max(1), RelocationTarget::Builtins);

    assert_eq!(
        observation.blocking_task.before, observation.blocking_task.after,
        "a shared single-threaded pool must keep dispatching blocking tasks to the same thread after relocating Builtins"
    );
    assert_async_follows_relocation(&observation);
}

#[test]
fn relocated_builtins_initializes_io_on_the_destination_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let initialized = Arc::new(Mutex::new(Vec::new()));
    let destination = runtime
        .scheduler()
        .block_on({
            let initialized = Arc::clone(&initialized);
            async move |cx| {
                let here = cx.thread().clone();
                let there = cx
                    .control()
                    .workers()
                    .into_iter()
                    .find(|worker| worker != &&here)
                    .cloned()
                    .expect("two-worker runtime exposes a relocation destination");
                let mut relocated = cx.clone();
                relocated.relocate(Some(&here), &there);
                let destination = there.id();
                let scheduler = relocated.scheduler().clone();
                scheduler
                    .spawn({
                        let options = RelocationIoOptions(initialized);
                        async move |_| {
                            assert_eq!(thread::current().id(), destination);
                            let _ = relocated.io().context::<RelocationIoContext>(&options);
                        }
                    })
                    .await
                    .unwrap();
                destination
            }
        })
        .unwrap();
    runtime.stop().unwrap();

    let initialized = initialized.lock().unwrap();
    assert_eq!(initialized.len(), 1);
    assert!(initialized.contains(&destination));
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
