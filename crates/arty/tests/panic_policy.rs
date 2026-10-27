// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime panic-policy contracts.

#![cfg(feature = "rt")]

mod support;

use std::error::Error as _;
use std::future::{Ready, pending};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Instant;

use arty::runtime::{BlockingPoolPolicy, PanicPolicy, Runtime, RuntimeBuilder, WorkersPolicy};
use arty::task::Builtins;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

fn runtime_builder(workers: usize, policy: PanicPolicy) -> RuntimeBuilder {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(workers))
        .blocking_pool(BlockingPoolPolicy::shared().max(2))
        .panic(policy)
}

fn shutdown_cause(error: &arty::runtime::Error) -> &arty_executor::ShutdownError {
    error
        .source()
        .unwrap()
        .source()
        .unwrap()
        .downcast_ref::<arty_executor::ShutdownError>()
        .unwrap()
}

fn panic_async(_: Builtins, (): ()) -> impl Future<Output = ()> {
    async { panic!("async panic") }
}

fn panic_factory(_: Builtins, (): ()) -> Ready<()> {
    panic!("factory panic")
}

fn pending_forever(_: Builtins, (): ()) -> impl Future<Output = ()> {
    pending()
}

fn return_value(_: Builtins, Unaware(value): Unaware<u32>) -> impl Future<Output = u32> {
    async move { value }
}

fn concurrent_panic(_: Builtins, Unaware(barrier): Unaware<Arc<Barrier>>) -> impl Future<Output = ()> {
    async move {
        barrier.wait();
        panic!("concurrent panic");
    }
}

fn detached_panic(_: Builtins, Unaware(panicking): Unaware<mpsc::Sender<()>>) -> impl Future<Output = ()> {
    async move {
        panicking.send(()).unwrap();
        panic!("detached panic");
    }
}

fn relocated_panic(Unaware(()): Unaware<()>) -> impl Future<Output = ()> {
    async { panic!("relocated panic") }
}

fn relocated_value(Unaware(value): Unaware<u32>) -> impl Future<Output = u32> {
    async move { value }
}

#[test]
fn default_async_panic_preserves_the_panic_join_and_starts_shutdown() {
    let runtime = runtime_builder(1, PanicPolicy::default()).build().unwrap();
    let task = runtime.scheduler().spawn_anywhere((), panic_async);

    let panic = task.join().unwrap_err();

    assert!(panic.is_panic());
    let rejected = runtime.scheduler().spawn_anywhere(Unaware(42), return_value).join().unwrap_err();
    assert!(rejected.is_shutdown());
    assert_eq!(
        rejected
            .source()
            .unwrap()
            .downcast_ref::<arty_executor::ShutdownError>()
            .unwrap()
            .inner()
            .to_string(),
        "a contained runtime operation panicked"
    );
    let stop = runtime.stop().unwrap_err();
    assert_eq!(shutdown_cause(&stop).inner().to_string(), "a contained runtime operation panicked");
}

#[test]
fn default_factory_panic_cancels_pending_work() {
    let runtime = runtime_builder(1, PanicPolicy::shutdown()).build().unwrap();
    let pending = runtime.scheduler().spawn_anywhere((), pending_forever);
    let panicking = runtime.scheduler().spawn_anywhere((), panic_factory);

    assert!(panicking.join().unwrap_err().is_panic());
    assert!(pending.join().unwrap_err().is_shutdown());
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "a contained runtime operation panicked"
    );
}

#[test]
fn default_blocking_panic_preserves_the_panic_join_and_starts_shutdown() {
    let runtime = runtime_builder(1, PanicPolicy::shutdown()).build().unwrap();
    let task = runtime.scheduler().spawn_blocking(|| panic!("blocking panic"));

    assert!(task.join().unwrap_err().is_panic());
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere(Unaware(42), return_value)
            .join()
            .unwrap_err()
            .is_shutdown()
    );
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "a contained runtime operation panicked"
    );
}

#[test]
fn ignore_policy_keeps_running_after_each_governed_panic_boundary() {
    let runtime = runtime_builder(1, PanicPolicy::ignore()).build().unwrap();
    let scheduler = runtime.scheduler();

    assert!(scheduler.spawn_anywhere((), panic_factory).join().unwrap_err().is_panic());
    assert_eq!(scheduler.spawn_anywhere(Unaware(1), return_value).join().unwrap(), 1);

    assert!(scheduler.spawn_anywhere((), panic_async).join().unwrap_err().is_panic());
    assert_eq!(scheduler.spawn_anywhere(Unaware(2), return_value).join().unwrap(), 2);

    assert!(scheduler.spawn_blocking(|| panic!("blocking panic")).join().unwrap_err().is_panic());
    assert_eq!(scheduler.spawn_anywhere(Unaware(3), return_value).join().unwrap(), 3);

    runtime.stop().unwrap();
}

#[test]
fn ignore_policy_remains_operational_across_workers_and_cloned_schedulers() {
    let runtime = runtime_builder(2, PanicPolicy::ignore()).build().unwrap();
    let destination = support::runtime_control(&runtime).workers().into_iter().nth(1).unwrap().clone();
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let cloned = scheduler.clone();

    assert!(
        scheduler
            .spawn_anywhere(Unaware(()), relocated_panic)
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(cloned.spawn(async |_| 42).join().unwrap(), 42);
    assert_eq!(cloned.spawn_at(&destination, Unaware(43), relocated_value).join().unwrap(), 43);

    runtime.stop().unwrap();
}

#[test]
fn concurrent_panics_start_one_coordinated_shutdown() {
    let runtime = runtime_builder(2, PanicPolicy::shutdown()).build().unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let tasks = [(), ()].map(|()| {
        let barrier = Arc::clone(&barrier);
        runtime.scheduler().spawn_anywhere(Unaware(barrier), concurrent_panic)
    });

    barrier.wait();

    for task in tasks {
        assert!(task.join().unwrap_err().is_panic());
    }
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "a contained runtime operation panicked"
    );
}

#[test]
fn panic_during_started_shutdown_keeps_the_first_shutdown_cause() {
    let runtime = runtime_builder(1, PanicPolicy::shutdown()).build().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let task = runtime.scheduler().spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        panic!("late blocking panic");
    });
    started_rx.recv().unwrap();
    let control = support::runtime_control(&runtime);

    control.request_stop();
    release_tx.send(()).unwrap();

    assert!(task.join().unwrap_err().is_panic());
    runtime.stop().unwrap();
}

#[test]
fn a_dropped_join_does_not_suppress_default_shutdown() {
    let runtime = runtime_builder(1, PanicPolicy::shutdown()).build().unwrap();
    let (panicking_tx, panicking_rx) = mpsc::channel();
    drop(runtime.scheduler().spawn_anywhere(Unaware(panicking_tx), detached_panic));
    panicking_rx.recv().unwrap();

    let deadline = Instant::now() + TEST_TIMEOUT;
    let rejected = loop {
        match runtime.scheduler().spawn_anywhere(Unaware(42), return_value).join() {
            Ok(_) => std::thread::yield_now(),
            Err(error) => break error,
        }
        assert!(Instant::now() < deadline, "panic policy did not start shutdown");
    };

    assert!(rejected.is_shutdown());
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "a contained runtime operation panicked"
    );
}
