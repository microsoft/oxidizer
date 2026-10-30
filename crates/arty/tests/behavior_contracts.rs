// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public construction, context, ownership, service, and join contracts.

#![cfg(feature = "rt")]
#![cfg(test)]

mod support;

use std::cell::Cell;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::task::{Context as PollContext, Poll, Waker};
use std::thread;

use arty::runtime::{BlockingPoolPolicy, PanicPolicy, Runtime, WorkersPolicy};
use arty::task::{Context, Scheduler};
use arty::time::Clock;
#[cfg(feature = "macros")]
use futures::future::join_all;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

fn runtime(workers: usize) -> Runtime {
    Runtime::builder().workers(WorkersPolicy::exactly(workers)).build().unwrap()
}

fn worker_scheduler(runtime: &Runtime) -> Scheduler {
    runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap()
}

fn count_workers(runtime: &Runtime) -> usize {
    let (scheduler, control) = runtime
        .scheduler()
        .block_on(async |cx| (cx.scheduler().clone(), cx.control().clone()))
        .unwrap();
    let tasks: Vec<_> = control
        .workers()
        .into_iter()
        .map(|worker| scheduler.spawn_at(worker, (), |()| async { thread::current().id() }))
        .collect();
    let count = tasks.len();
    let workers: std::collections::HashSet<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
    assert_eq!(workers.len(), count);
    count
}

fn completes_without_deadlock(body: impl FnOnce() + Send + 'static) {
    let (completed, completion) = mpsc::channel();
    thread::spawn(move || {
        body();
        _ = completed.send(());
    });
    if completion.recv_timeout(TEST_TIMEOUT).is_err() {
        eprintln!("the worker wait guard deadlocked");
        #[expect(clippy::exit, reason = "a deadlocked runtime thread prevents normal test-process teardown")]
        std::process::exit(112);
    }
}

struct BlockingGate {
    released: Mutex<bool>,
    changed: Condvar,
}

impl BlockingGate {
    fn wait(&self) {
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

struct ReleaseBlockingGate(Arc<BlockingGate>);

impl Drop for ReleaseBlockingGate {
    fn drop(&mut self) {
        self.0.release();
    }
}

fn assert_blocking_pool_limit(max_threads: usize) {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(max_threads))
        .build()
        .unwrap();
    let gate = Arc::new(BlockingGate {
        released: Mutex::new(false),
        changed: Condvar::new(),
    });
    let _release_gate = ReleaseBlockingGate(Arc::clone(&gate));
    let scheduler = runtime.scheduler();
    let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
    let blocker = scheduler.spawn_blocking({
        let gate = Arc::clone(&gate);
        move || {
            let worker = thread::current().id();
            blocker_started_tx.send(worker).unwrap();
            gate.wait();
            worker
        }
    });
    let blocker_worker = blocker_started_rx.recv_timeout(TEST_TIMEOUT).unwrap();

    let (queued_started_tx, queued_started_rx) = mpsc::channel();
    // Six queued tasks exceed the pool's five-task-per-thread growth threshold.
    let mut queued = Vec::with_capacity(6);
    for _ in 0..6 {
        let gate = Arc::clone(&gate);
        let queued_started_tx = queued_started_tx.clone();
        queued.push(scheduler.spawn_blocking(move || {
            let worker = thread::current().id();
            queued_started_tx.send(worker).unwrap();
            gate.wait();
            worker
        }));
    }
    drop(queued_started_tx);

    if max_threads > 1 {
        let growth_worker = queued_started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_ne!(growth_worker, blocker_worker);
    }

    gate.release();
    let mut worker_ids = HashSet::new();
    worker_ids.insert(blocker.join().unwrap());
    worker_ids.extend(queued.into_iter().map(|task| task.join().unwrap()));
    runtime.stop().unwrap();

    assert_eq!(worker_ids.len(), max_threads);
}

#[test]
fn public_policy_defaults_are_automatic_and_shared() {
    assert_eq!(BlockingPoolPolicy::default(), BlockingPoolPolicy::shared());
}

#[test]
fn blocking_pool_grows_when_overloaded_below_its_limit() {
    assert_blocking_pool_limit(2);
}

#[test]
fn blocking_pool_growth_respects_its_configured_limit() {
    assert_blocking_pool_limit(1);
}

#[test]
fn the_last_processor_setting_replaces_invalid_and_previous_counts() {
    for previous in [
        WorkersPolicy::exactly(0),
        WorkersPolicy::at_most(0),
        WorkersPolicy::exactly(usize::MAX),
        WorkersPolicy::exactly(2),
        WorkersPolicy::all(),
        WorkersPolicy::default(),
    ] {
        let runtime = Runtime::builder()
            .workers(previous)
            .workers(WorkersPolicy::at_most(1))
            .build()
            .unwrap();
        assert_eq!(count_workers(&runtime), 1);
        runtime.stop().unwrap();
    }
}

#[test]
#[should_panic(expected = "stack size must be greater than zero")]
fn zero_stack_size_is_rejected_by_the_setter() {
    let _ = Runtime::builder().stack_size(0);
}

#[test]
fn resource_setters_preserve_worker_count_and_pool_policy() {
    let runtime = Runtime::builder()
        .blocking_pool(BlockingPoolPolicy::per_worker())
        .workers(WorkersPolicy::at_most(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .workers(WorkersPolicy::exactly(2))
        .build()
        .unwrap();
    assert_eq!(count_workers(&runtime), 2);
    let first = worker_scheduler(&runtime);
    let second = worker_scheduler(&runtime);
    assert_eq!(
        first.spawn_blocking(|| thread::current().id()).join().unwrap(),
        second.spawn_blocking(|| thread::current().id()).join().unwrap()
    );
    runtime.stop().unwrap();
}

#[test]
fn construction_and_shutdown_work_inside_a_futures_executor() {
    futures::executor::block_on(async {
        let runtime = runtime(1);
        let (answer, scheduler) = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move {
                assert_eq!(cx.thread().id(), thread::current().id());
                (42, cx.scheduler().clone())
            })
            .await
            .unwrap();
        assert_eq!(answer, 42);
        runtime.stop().unwrap();
        assert!(scheduler.spawn(async |_| ()).await.unwrap_err().is_shutdown());
    });
}

#[test]
fn explicit_stop_on_a_task_worker_panics() {
    completes_without_deadlock(|| {
        let runtime = runtime(1);
        let scheduler = worker_scheduler(&runtime);
        assert!(
            scheduler
                .spawn(async |cx| cx.thread().id() == thread::current().id())
                .join()
                .unwrap()
        );
        let error = scheduler.spawn(async move |_| runtime.stop()).join().unwrap_err();
        assert!(error.is_panic());
    });
}

#[test]
fn explicit_stop_on_a_blocking_worker_panics() {
    let runtime = runtime(1);
    let scheduler = worker_scheduler(&runtime);
    assert_ne!(
        scheduler.spawn_blocking(|| thread::current().id()).join().unwrap(),
        scheduler.spawn(async |_| thread::current().id()).join().unwrap()
    );
    let error = scheduler.spawn_blocking(move || runtime.stop()).join().unwrap_err();
    assert!(error.is_panic());
}

#[test]
fn nested_block_on_rejects_the_factory_before_submission() {
    let runtime = runtime(1);
    let invoked = AtomicBool::new(false);
    let outcome = futures::executor::block_on(async {
        runtime.scheduler().block_on(|_| {
            invoked.store(true, Ordering::Relaxed);
            async { 42 }
        })
    });
    outcome.unwrap_err();
    assert!(!invoked.load(Ordering::Relaxed));
    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn worker_context_belongs_to_one_runtime_and_rejects_nested_block_on() {
    completes_without_deadlock(|| {
        let first = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .build()
            .unwrap();
        let second = runtime(1);
        let first_scheduler = first.scheduler();
        let second_scheduler = second.scheduler();
        first_scheduler
            .block_on(async |_| {
                let invoked = AtomicBool::new(false);
                for scheduler in [first_scheduler, second_scheduler] {
                    scheduler
                        .block_on(|_| {
                            invoked.store(true, Ordering::Relaxed);
                            async { 42 }
                        })
                        .unwrap_err();
                }
                assert!(!invoked.load(Ordering::Relaxed));
            })
            .unwrap();
        assert_eq!(second_scheduler.block_on(async |_| 42).unwrap(), 42);
        first.stop().unwrap();
        second.stop().unwrap();
    });
}

#[test]
fn blocking_callbacks_reject_block_on_before_factory_submission() {
    let runtime = Arc::new(runtime(1));
    let captured = Arc::clone(&runtime);
    let invoked = Arc::new(AtomicBool::new(false));
    let invoked_by_factory = Arc::clone(&invoked);
    let error = runtime
        .scheduler()
        .spawn_blocking(move || {
            captured.scheduler().block_on(move |_| {
                invoked_by_factory.store(true, Ordering::Relaxed);
                async { 42 }
            })
        })
        .join()
        .unwrap()
        .unwrap_err();

    assert_eq!(error.to_string(), "block_on cannot be called from an Arty worker");
    assert!(!invoked.load(Ordering::Relaxed));
    Arc::try_unwrap(runtime).unwrap().stop().unwrap();
}

#[test]
fn context_publishes_scheduler_and_clock_on_every_worker() {
    let runtime = runtime(2);
    let context = runtime.scheduler().block_on(async |cx| cx).unwrap();
    let tasks: Vec<_> = context
        .control()
        .workers()
        .into_iter()
        .map(|worker| {
            context.scheduler().spawn_at(worker, context.clone(), |cx: Context| async move {
                let scheduler: &Scheduler = cx.as_ref();
                let clock: &Clock = cx.as_ref();
                assert!(std::ptr::eq(scheduler, cx.scheduler()));
                assert!(std::ptr::eq(clock, cx.clock()));
                assert_eq!(cx.thread().id(), thread::current().id());
                assert_eq!(scheduler.spawn(async |_| 42).await.unwrap(), 42);
                cx.thread().id()
            })
        })
        .collect();
    assert_eq!(tasks.len(), 2);
    let workers: std::collections::HashSet<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
    assert_eq!(workers.len(), 2);
    runtime.stop().unwrap();
}

#[cfg(all(feature = "macros", not(miri)))]
#[arty::test(workers = 2)]
async fn worker_services_are_ready_before_spawning(cx: Context) {
    let scheduler = cx.scheduler();
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            scheduler.spawn_anywhere(cx.clone(), |worker: Context| async move {
                worker.thread().id() == thread::current().id()
            })
        })
        .collect();
    let expected = vec![true; tasks.len()];
    let actual = join_all(tasks).await.into_iter().map(Result::unwrap).collect::<Vec<_>>();

    assert_eq!(actual, expected);
}

#[cfg(feature = "test-util")]
#[test]
fn configured_clock_control_remains_the_worker_time_source() {
    use std::time::Duration;

    use arty::time::ClockControl;

    let control = ClockControl::new();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .clock(control.clone())
        .build()
        .unwrap();
    let before = runtime.scheduler().block_on(async |cx| cx.clock().system_time()).unwrap();
    control.advance(Duration::from_secs(1));
    let after = runtime.scheduler().block_on(async |cx| cx.clock().system_time()).unwrap();
    assert_eq!(after.duration_since(before).unwrap(), Duration::from_secs(1));
    runtime.stop().unwrap();
}

#[test]
fn default_blocking_pools_are_shared_between_workers() {
    let runtime = runtime(2);
    let schedulers: Vec<_> = (0..2).map(|_| worker_scheduler(&runtime)).collect();
    let workers: Vec<_> = schedulers
        .iter()
        .map(|scheduler| scheduler.spawn(async |cx| cx.thread().id()).join().unwrap())
        .collect();
    assert_ne!(workers[0], workers[1]);
    let blocking: Vec<_> = schedulers
        .iter()
        .map(|scheduler| scheduler.spawn_blocking(|| thread::current().id()).join().unwrap())
        .collect();
    assert_eq!(blocking[0], blocking[1]);
    assert!(!workers.contains(&blocking[0]));
    runtime.stop().unwrap();
}

#[test]
fn stopping_one_runtime_does_not_stop_another() {
    let first = runtime(1);
    let second = runtime(1);
    let first_scheduler = worker_scheduler(&first);
    let second_scheduler = worker_scheduler(&second);
    let first_worker = first_scheduler.spawn(async |cx| cx.thread().id()).join().unwrap();
    let second_worker = second_scheduler.spawn(async |cx| cx.thread().id()).join().unwrap();
    assert_ne!(first_worker, second_worker);
    support::runtime_control(&first).request_stop();
    first.stop().unwrap();
    assert!(first_scheduler.spawn(async |_| 42).join().unwrap_err().is_shutdown());
    assert_eq!(second_scheduler.spawn(async |_| 42).join().unwrap(), 42);
    second.stop().unwrap();
}

#[test]
fn dropping_another_runtime_from_a_worker_panics() {
    let target = runtime(1);
    let caller = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .panic(PanicPolicy::ignore())
        .build()
        .unwrap();
    let retained = worker_scheduler(&target);
    let (started, ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let callback = target.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        gate.recv_timeout(TEST_TIMEOUT).unwrap();
        42
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let error = caller
        .scheduler()
        .spawn_anywhere(Unaware(target), |_, Unaware(target)| async move { drop(target) })
        .join()
        .unwrap_err();
    assert!(error.is_panic());
    assert!(retained.spawn(async |_| 42).join().unwrap_err().is_shutdown());
    release.send(()).unwrap();
    assert_eq!(callback.join().unwrap(), 42);
    caller.stop().unwrap();
}

#[cfg(feature = "macros")]
#[arty::test]
async fn application_errors_remain_results_for_async_and_blocking_tasks(cx: Context) {
    assert_eq!(
        cx.scheduler().spawn(async |_| Err::<(), _>("application")).await.unwrap(),
        Err("application")
    );
    assert_eq!(
        cx.scheduler().spawn_blocking(|| Err::<(), _>("application")).await.unwrap(),
        Err("application")
    );
}

#[test]
fn remote_join_accepts_a_send_only_result_and_rejects_repolling() {
    static_assertions::assert_impl_all!(arty::task::JoinHandle<Cell<u32>>: Send);
    let runtime = runtime(1);
    let scheduler = worker_scheduler(&runtime);
    let (completed, observed) = mpsc::channel();
    let mut task = Box::pin(scheduler.spawn(async move |_| {
        completed.send(()).unwrap();
        Cell::new(42)
    }));
    observed.recv_timeout(TEST_TIMEOUT).unwrap();
    // This factory cannot run on the same worker until the signalled poll returns.
    scheduler.spawn(async |_| ()).join().unwrap();
    let Poll::Ready(Ok(value)) = task.as_mut().poll(&mut PollContext::from_waker(Waker::noop())) else {
        panic!("an already-completed task must be immediately ready");
    };
    assert_eq!(value.get(), 42);
    catch_unwind(AssertUnwindSafe(|| task.as_mut().poll(&mut PollContext::from_waker(Waker::noop())))).unwrap_err();
    assert_eq!(
        runtime.scheduler().spawn_anywhere((), |_, ()| async { 123u32 }).join().unwrap(),
        123
    );
    runtime.stop().unwrap();
}

#[test]
fn rejected_remote_joins_report_shutdown_then_reject_repolling() {
    let runtime = runtime(1);
    support::runtime_control(&runtime).request_stop();
    let mut task = Box::pin(runtime.scheduler().spawn_anywhere((), |_, ()| async {}));
    let Poll::Ready(Err(error)) = task.as_mut().poll(&mut PollContext::from_waker(Waker::noop())) else {
        panic!("a rejected submission must be immediately ready");
    };
    assert!(error.is_shutdown());
    assert!(!error.is_panic());
    catch_unwind(AssertUnwindSafe(|| task.as_mut().poll(&mut PollContext::from_waker(Waker::noop())))).unwrap_err();
    runtime.stop().unwrap();
}

#[cfg(feature = "macros")]
mod entry_points {
    use arty::task::Context;

    #[arty::main(workers = 1)]
    async fn application_error(_cx: Context) -> Result<(), &'static str> {
        Err("application")
    }

    #[test]
    fn main_preserves_application_errors_as_return_values() {
        assert_eq!(application_error(), Err("application"));
    }
}
