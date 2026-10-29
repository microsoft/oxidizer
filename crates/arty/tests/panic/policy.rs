// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime panic-policy contracts.

#![cfg(feature = "rt")]

use std::error::Error as _;
use std::future::{Ready, pending};
use std::pin::Pin;
use std::sync::{Arc, Barrier, mpsc};
use std::task::{Context, Wake, Waker};
use std::time::Instant;

use arty::runtime::{BlockingPoolPolicy, PanicPolicy, Runtime, RuntimeBuilder, RuntimeControl, WorkersPolicy};
use arty::task::Builtins;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

use super::support;
use super::support::JoinHandleExt as _;

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

async fn panic_async(_: Builtins, (): ()) {
    std::panic::panic_any("async panic");
}

fn panic_factory(_: Builtins, (): ()) -> Ready<()> {
    std::panic::panic_any("factory panic")
}

fn pending_forever(_: Builtins, (): ()) -> impl Future<Output = ()> {
    pending()
}

async fn return_value(_: Builtins, Unaware(value): Unaware<u32>) -> u32 {
    value
}

async fn concurrent_panic(_: Builtins, Unaware(barrier): Unaware<Arc<Barrier>>) {
    barrier.wait();
    std::panic::panic_any("concurrent panic");
}

async fn detached_panic(_: Builtins, Unaware(panicking): Unaware<mpsc::Sender<()>>) {
    panicking.send(()).unwrap();
    std::panic::panic_any("detached panic");
}

async fn relocated_panic(Unaware(()): Unaware<()>) {
    std::panic::panic_any("relocated panic");
}

async fn relocated_value(Unaware(value): Unaware<u32>) -> u32 {
    value
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
fn panic_completion_records_its_cause_before_waking_the_joiner() {
    assert_panic_cause_precedes_join_notification(true, false);
}

#[test]
fn panic_cancellation_records_its_cause_before_waking_the_joiner() {
    assert_panic_cause_precedes_join_notification(false, false);
}

#[test]
fn blocking_panic_completion_records_its_cause_before_waking_the_joiner() {
    assert_panic_cause_precedes_join_notification(true, true);
}

#[test]
fn blocking_panic_cancellation_records_its_cause_before_waking_the_joiner() {
    assert_panic_cause_precedes_join_notification(false, true);
}

fn assert_panic_cause_precedes_join_notification(observe_panic: bool, blocking: bool) {
    struct StopOnWake {
        control: RuntimeControl,
        notified: mpsc::Sender<()>,
    }

    impl Wake for StopOnWake {
        fn wake(self: Arc<Self>) {
            self.control.request_stop();
            self.notified.send(()).unwrap();
        }
    }

    let runtime = runtime_builder(1, PanicPolicy::shutdown()).build().unwrap();
    let control = support::runtime_control(&runtime);
    let (started_tx, started_rx) = mpsc::channel();
    let pending = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started_tx), |_, Unaware(started)| async move {
            started.send(()).unwrap();
            pending::<()>().await;
        });
    started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    let (release, released) = events_once::Event::<()>::boxed();
    let panicking: arty::task::JoinHandle<()> = if blocking {
        runtime.scheduler().spawn_blocking(move || {
            futures::executor::block_on(released).unwrap();
            panic!("panic before a join notification");
        })
    } else {
        runtime
            .scheduler()
            .spawn_anywhere(Unaware(released), |_, Unaware(released)| async move {
                released.await.unwrap();
                panic!("panic before a join notification");
            })
    };
    let mut observed = if observe_panic { panicking } else { pending };
    let (notified, notifications) = mpsc::channel();
    let waker = Waker::from(Arc::new(StopOnWake { control, notified }));
    assert!(Pin::new(&mut observed).poll(&mut Context::from_waker(&waker)).is_pending());

    release.send(());
    notifications.recv_timeout(TEST_TIMEOUT).unwrap();

    let error = observed.join().unwrap_err();
    assert_eq!(error.is_panic(), observe_panic);
    assert_eq!(error.is_shutdown(), !observe_panic);
    let stop = runtime.stop().unwrap_err();
    let expected = shutdown_cause(&stop);
    assert_eq!(expected.inner().to_string(), "a contained runtime operation panicked");
    if !observe_panic {
        let source = error.source().unwrap().downcast_ref::<arty_executor::ShutdownError>().unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }
}

#[test]
fn blocking_panics_follow_the_configured_policy() {
    for (policy, starts_shutdown) in [(PanicPolicy::default(), true), (PanicPolicy::ignore(), false)] {
        let runtime = runtime_builder(1, policy).build().unwrap();
        let (release, released) = events_once::Event::<()>::boxed();
        let pending = runtime
            .scheduler()
            .spawn_anywhere(Unaware(released), |_, Unaware(released)| async move {
                released.await.unwrap();
                7
            });
        let task: arty::task::JoinHandle<()> = runtime.scheduler().spawn_blocking(|| std::panic::panic_any(42_u32));

        let panic = task.join().unwrap_err();
        assert!(panic.is_panic());
        assert_eq!(*panic.into_panic().unwrap().downcast::<u32>().unwrap(), 42);
        if starts_shutdown {
            assert!(
                runtime
                    .scheduler()
                    .spawn_anywhere(Unaware(43), return_value)
                    .join()
                    .unwrap_err()
                    .is_shutdown()
            );
            assert!(runtime.scheduler().spawn_blocking(|| 44).join().unwrap_err().is_shutdown());
            let cancellation = pending.join().unwrap_err();
            assert!(cancellation.is_shutdown());
            let stop = runtime.stop().unwrap_err();
            let expected = shutdown_cause(&stop);
            assert_eq!(expected.inner().to_string(), "a contained runtime operation panicked");
            let actual = cancellation
                .source()
                .unwrap()
                .downcast_ref::<arty_executor::ShutdownError>()
                .unwrap();
            assert!(std::ptr::addr_eq(actual.inner(), expected.inner()));
        } else {
            assert_eq!(runtime.scheduler().spawn_anywhere(Unaware(43), return_value).join().unwrap(), 43);
            assert_eq!(runtime.scheduler().spawn_blocking(|| 44).join().unwrap(), 44);
            release.send(());
            assert_eq!(pending.join().unwrap(), 7);
            runtime.stop().unwrap();
        }
    }
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

#[test]
fn detached_async_panic_cascades_its_shutdown_cause_to_every_worker() {
    let runtime = runtime_builder(2, PanicPolicy::shutdown()).build().unwrap();
    let control = support::runtime_control(&runtime);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let pending: Vec<_> = control
        .workers()
        .into_iter()
        .map(|worker| {
            scheduler.spawn_at(worker, Unaware(started_tx.clone()), |Unaware(started)| async move {
                started.send(()).unwrap();
                pending::<()>().await;
            })
        })
        .collect();
    for _ in &pending {
        started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    }

    drop(scheduler.spawn_at(control.workers().into_iter().next().unwrap(), Unaware(()), relocated_panic));

    let cancellations: Vec<_> = pending.into_iter().map(|task| task.join().unwrap_err()).collect();
    let stop = runtime.stop().unwrap_err();
    let expected = shutdown_cause(&stop);
    assert_eq!(expected.inner().to_string(), "a contained runtime operation panicked");
    for cancellation in cancellations {
        assert!(cancellation.is_shutdown());
        let source = cancellation
            .source()
            .unwrap()
            .downcast_ref::<arty_executor::ShutdownError>()
            .unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }
}

#[test]
fn detached_blocking_panic_cascades_its_shutdown_cause_to_every_worker() {
    let runtime = runtime_builder(2, PanicPolicy::shutdown()).build().unwrap();
    let control = support::runtime_control(&runtime);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let pending: Vec<_> = control
        .workers()
        .into_iter()
        .map(|worker| {
            scheduler.spawn_at(worker, Unaware(started_tx.clone()), |Unaware(started)| async move {
                started.send(()).unwrap();
                pending::<()>().await;
            })
        })
        .collect();
    for _ in &pending {
        started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    }
    let (release, released) = mpsc::channel();
    drop(scheduler.spawn_blocking(move || {
        released.recv_timeout(TEST_TIMEOUT).unwrap();
        panic!("detached blocking panic");
    }));
    release.send(()).unwrap();

    let cancellations: Vec<_> = pending.into_iter().map(|task| task.join().unwrap_err()).collect();
    let stop = runtime.stop().unwrap_err();
    let expected = shutdown_cause(&stop);
    assert_eq!(expected.inner().to_string(), "a contained runtime operation panicked");
    for cancellation in cancellations {
        assert!(cancellation.is_shutdown());
        let actual = cancellation
            .source()
            .unwrap()
            .downcast_ref::<arty_executor::ShutdownError>()
            .unwrap();
        assert!(std::ptr::addr_eq(actual.inner(), expected.inner()));
    }
}

#[test]
fn detached_blocking_panic_keeps_work_running_under_ignore_policy() {
    let runtime = runtime_builder(2, PanicPolicy::ignore())
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .build()
        .unwrap();
    let control = support::runtime_control(&runtime);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let pending: Vec<_> = control
        .workers()
        .into_iter()
        .map(|worker| {
            let (release, released) = events_once::Event::<()>::boxed();
            let task = scheduler.spawn_at(
                worker,
                Unaware((started_tx.clone(), released)),
                |Unaware((started, released))| async move {
                    started.send(()).unwrap();
                    released.await.unwrap();
                    42
                },
            );
            (release, task)
        })
        .collect();
    for _ in &pending {
        started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    }

    drop(scheduler.spawn_blocking(|| panic!("detached blocking panic")));
    assert_eq!(scheduler.spawn_blocking(|| 7).join().unwrap(), 7);

    for (release, task) in pending {
        release.send(());
        assert_eq!(task.join().unwrap(), 42);
    }
    runtime.stop().unwrap();
}
