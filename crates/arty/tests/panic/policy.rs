// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime panic-policy contracts.

#![cfg(feature = "rt")]

use std::error::Error as _;
use std::future::{Ready, pending};
use std::pin::Pin;
use std::sync::{Arc, Barrier, mpsc};
use std::task::{Context as PollContext, Wake, Waker};

use arty::runtime::hook::{HookFn, OnPanic};
use arty::runtime::{BlockingPoolPolicy, Runtime, RuntimeBuilder, RuntimeControl, ShutdownCause, WorkersPolicy};
use arty::task::Context;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

use crate::support;
use crate::support::JoinHandleExt as _;

#[derive(Clone, Copy)]
enum PanicResponse {
    Shutdown,
    Ignore,
}

fn runtime_builder(workers: usize, response: PanicResponse) -> RuntimeBuilder {
    let builder = Runtime::builder()
        .workers(WorkersPolicy::exactly(workers))
        // Two threads keep blocking-work scenarios bounded without serializing
        // callbacks whose completion or cancellation the test observes.
        .blocking_pool(BlockingPoolPolicy::shared().max(2));
    match response {
        PanicResponse::Shutdown => builder.on(HookFn::<OnPanic, _>::new((), |event, ()| event.shutdown())),
        PanicResponse::Ignore => builder.on(OnPanic::IGNORE),
    }
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

async fn panic_async(_: Context, (): ()) {
    std::panic::panic_any("async panic");
}

fn panic_factory(_: Context, (): ()) -> Ready<()> {
    std::panic::panic_any("factory panic")
}

fn pending_forever(_: Context, (): ()) -> impl Future<Output = ()> {
    pending()
}

async fn return_value(_: Context, Unaware(value): Unaware<u32>) -> u32 {
    value
}

async fn concurrent_panic(Unaware(barrier): Unaware<Arc<Barrier>>) {
    barrier.wait();
    std::panic::panic_any("concurrent panic");
}

async fn detached_panic(_: Context, Unaware(panicking): Unaware<mpsc::Sender<()>>) {
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
    let runtime = runtime_builder(1, PanicResponse::Shutdown).build().unwrap();
    let task = runtime.scheduler().spawn_anywhere((), panic_async);

    let panic = task.join().unwrap_err();

    assert!(panic.is_panic());
    assert!(!panic.is_shutdown());
    assert_eq!(panic.shutdown_cause(), None);
    assert!(panic.source().is_none());
    assert_eq!(*panic.into_panic().unwrap().downcast::<&str>().unwrap(), "async panic");
    let rejected = runtime.scheduler().spawn_anywhere(Unaware(42), return_value).join().unwrap_err();
    assert!(rejected.is_shutdown());
    assert!(!rejected.is_panic());
    assert_eq!(rejected.shutdown_cause(), Some(ShutdownCause::Panic));
    assert_eq!(
        rejected.source().unwrap().to_string(),
        "runtime shutdown was caused by a task panic: async panic"
    );
    let stop = runtime.stop().unwrap_err();
    assert_eq!(
        shutdown_cause(&stop).inner().to_string(),
        "runtime shutdown was caused by a task panic: async panic"
    );
}

#[test]
fn original_async_panic_message_reaches_cancelled_and_rejected_join_errors() {
    let runtime = runtime_builder(1, PanicResponse::Shutdown).build().unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let pending = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started_tx), |_, Unaware(started)| async move {
            started.send(()).unwrap();
            pending::<()>().await;
        });
    started_rx.recv_timeout(TEST_TIMEOUT).unwrap();

    let task: arty::task::JoinHandle<()> = runtime
        .scheduler()
        .spawn_anywhere(Unaware(String::from("owned async panic")), |_, Unaware(message)| async move {
            std::panic::panic_any(message)
        });

    let panic = task.join().unwrap_err();
    assert!(panic.is_panic());
    assert_eq!(panic.shutdown_cause(), None);
    assert!(panic.source().is_none());
    assert_eq!(*panic.into_panic().unwrap().downcast::<String>().unwrap(), "owned async panic");

    let cancelled = pending.join().unwrap_err();
    let rejected_async = runtime.scheduler().spawn_anywhere(Unaware(42), return_value).join().unwrap_err();
    let rejected_blocking = runtime.scheduler().spawn_blocking(|| 42).join().unwrap_err();
    for error in [cancelled, rejected_async, rejected_blocking] {
        assert!(error.is_shutdown());
        assert!(!error.is_panic());
        assert_eq!(error.shutdown_cause(), Some(ShutdownCause::Panic));
        assert_eq!(
            error.source().unwrap().to_string(),
            "runtime shutdown was caused by a task panic: owned async panic"
        );
        assert!(error.into_panic().is_none());
    }
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).to_string(),
        "runtime shutdown was caused by a task panic: owned async panic"
    );
}

#[test]
fn default_factory_panic_cancels_pending_work() {
    let runtime = runtime_builder(1, PanicResponse::Shutdown).build().unwrap();
    let pending = runtime.scheduler().spawn_anywhere((), pending_forever);
    let panicking = runtime.scheduler().spawn_anywhere((), panic_factory);

    assert!(panicking.join().unwrap_err().is_panic());
    let cancellation = pending.join().unwrap_err();
    assert!(cancellation.is_shutdown());
    assert_eq!(cancellation.shutdown_cause(), Some(ShutdownCause::Panic));
    assert_eq!(
        cancellation.source().unwrap().to_string(),
        "runtime shutdown was caused by a task panic: factory panic"
    );
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "runtime shutdown was caused by a task panic: factory panic"
    );
}

#[test]
fn panic_completion_records_its_cause_before_waking_the_joiner() {
    assert_panic_cause_precedes_join_notification(true);
}

#[test]
fn panic_cancellation_records_its_cause_before_waking_the_joiner() {
    assert_panic_cause_precedes_join_notification(false);
}

fn assert_panic_cause_precedes_join_notification(observe_panic: bool) {
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

    let runtime = runtime_builder(1, PanicResponse::Shutdown).build().unwrap();
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
    let panicking: arty::task::JoinHandle<()> = runtime
        .scheduler()
        .spawn_anywhere(Unaware(released), |_, Unaware(released)| async move {
            released.await.unwrap();
            panic!("panic before a join notification");
        });
    let mut observed = if observe_panic { panicking } else { pending };
    let (notified, notifications) = mpsc::channel();
    let waker = Waker::from(Arc::new(StopOnWake { control, notified }));
    assert!(Pin::new(&mut observed).poll(&mut PollContext::from_waker(&waker)).is_pending());

    release.send(());
    notifications.recv_timeout(TEST_TIMEOUT).unwrap();

    let error = observed.join().unwrap_err();
    assert_eq!(error.is_panic(), observe_panic);
    assert_eq!(error.is_shutdown(), !observe_panic);
    let stop = runtime.stop().unwrap_err();
    let expected = shutdown_cause(&stop);
    assert_eq!(
        expected.inner().to_string(),
        "runtime shutdown was caused by a task panic: panic before a join notification"
    );
    if !observe_panic {
        assert_eq!(error.shutdown_cause(), Some(ShutdownCause::Panic));
        let source = error.source().unwrap().downcast_ref::<arty_executor::ShutdownError>().unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }
}

#[test]
fn blocking_panics_do_not_run_hooks_or_start_shutdown() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<OnPanic, _>::new(Unaware(Arc::clone(&calls)), |event, data| {
            data.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            event.shutdown();
        }))
        .build()
        .unwrap();

    let panic = runtime
        .scheduler()
        .spawn_blocking(|| std::panic::panic_any(42_u32))
        .join()
        .unwrap_err();

    assert!(panic.is_panic());
    assert_eq!(*panic.into_panic().unwrap().downcast::<u32>().unwrap(), 42);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(runtime.scheduler().spawn_anywhere(Unaware(43), return_value).join().unwrap(), 43);
    assert_eq!(runtime.scheduler().spawn_blocking(|| 44).join().unwrap(), 44);
    runtime.stop().unwrap();
}

#[test]
fn ignore_policy_keeps_running_after_each_governed_panic_boundary() {
    let runtime = runtime_builder(1, PanicResponse::Ignore).build().unwrap();
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
    let runtime = runtime_builder(2, PanicResponse::Ignore).build().unwrap();
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
    let runtime = runtime_builder(2, PanicResponse::Shutdown).build().unwrap();
    let control = support::runtime_control(&runtime);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let tasks: Vec<_> = control
        .workers()
        .into_iter()
        .map(|worker| {
            let barrier = Arc::clone(&barrier);
            scheduler.spawn_at(worker, Unaware(barrier), concurrent_panic)
        })
        .collect();

    barrier.wait();

    for task in tasks {
        assert!(task.join().unwrap_err().is_panic());
    }
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "runtime shutdown was caused by a task panic: concurrent panic"
    );
}

#[test]
fn panic_during_started_shutdown_keeps_the_first_shutdown_cause() {
    let runtime = runtime_builder(1, PanicResponse::Shutdown).build().unwrap();
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
    let rejected = runtime.scheduler().spawn_blocking(|| 42).join().unwrap_err();
    assert_eq!(rejected.shutdown_cause(), Some(ShutdownCause::Requested));
    assert!(rejected.source().is_none());
    runtime.stop().unwrap();
}

#[test]
fn a_dropped_join_does_not_suppress_default_shutdown() {
    let runtime = runtime_builder(1, PanicResponse::Shutdown).build().unwrap();
    let (panicking_tx, panicking_rx) = mpsc::channel();
    drop(runtime.scheduler().spawn_anywhere(Unaware(panicking_tx), detached_panic));
    panicking_rx.recv().unwrap();

    let rejected = runtime.scheduler().spawn_anywhere(Unaware(42), return_value).join().unwrap_err();

    assert!(rejected.is_shutdown());
    assert_eq!(rejected.shutdown_cause(), Some(ShutdownCause::Panic));
    assert_eq!(
        shutdown_cause(&runtime.stop().unwrap_err()).inner().to_string(),
        "runtime shutdown was caused by a task panic: detached panic"
    );
}

#[test]
fn detached_async_panic_cascades_its_shutdown_cause_to_every_worker() {
    let runtime = runtime_builder(2, PanicResponse::Shutdown).build().unwrap();
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
    assert_eq!(
        expected.inner().to_string(),
        "runtime shutdown was caused by a task panic: relocated panic"
    );
    for cancellation in cancellations {
        assert!(cancellation.is_shutdown());
        assert_eq!(cancellation.shutdown_cause(), Some(ShutdownCause::Panic));
        let source = cancellation
            .source()
            .unwrap()
            .downcast_ref::<arty_executor::ShutdownError>()
            .unwrap();
        assert!(std::ptr::addr_eq(source.inner(), expected.inner()));
    }
}

#[test]
fn detached_blocking_panic_keeps_runtime_running() {
    let runtime = runtime_builder(2, PanicResponse::Shutdown)
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
