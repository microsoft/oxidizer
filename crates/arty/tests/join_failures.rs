// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Task failure and shutdown are results, not unwinds or permanently pending joins.

use std::future::{pending, ready};
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};
use arty::task::{JoinError, JoinHandle};
use testing_aids::{TEST_TIMEOUT, execute_or_terminate_process};

testing_aids::init_tracing!();

#[cfg(test)]
fn runtime() -> Runtime {
    Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
        .build()
        .unwrap()
}

#[test]
fn async_and_blocking_panics_do_not_unwind_the_joiner() {
    let runtime = runtime();
    let scheduler = runtime.task_scheduler();
    let handles: [JoinHandle<()>; 3] = [
        scheduler.spawn(|_| -> std::future::Ready<()> { panic!("factory panic") }),
        scheduler.spawn(async |_| panic!("poll panic")),
        scheduler.spawn_blocking(|| panic!("blocking panic")),
    ];
    for handle in handles {
        let result = catch_unwind(AssertUnwindSafe(|| handle.wait())).unwrap();
        let error = result.unwrap_err();
        assert!(error.is_panic());
        assert!(!error.is_shutdown());
    }
    assert_eq!(scheduler.spawn(async |_| 42).wait().unwrap(), 42);
}

#[test]
fn local_factory_and_poll_panics_are_join_errors() {
    runtime()
        .run(async |cx| {
            let scheduler = cx.local_scheduler().unwrap();
            let factory = catch_unwind(AssertUnwindSafe(|| {
                scheduler.spawn(|| -> std::future::Ready<()> { panic!("local factory panic") })
            }))
            .unwrap();
            assert!(factory.await.unwrap_err().is_panic());
            assert!(scheduler.spawn(async || panic!("local poll panic")).await.unwrap_err().is_panic());
            let value = scheduler.spawn(async || Rc::new(42)).await.unwrap();
            assert_eq!(*value, 42);
        })
        .unwrap();
}

#[test]
fn stop_rejects_remote_blocking_and_local_factories_immediately() {
    runtime()
        .run(async |cx| {
            let local = cx.local_scheduler().unwrap();
            cx.runtime_operations().stop();
            let invoked = Arc::new(AtomicBool::new(false));
            let remote = cx.scheduler().spawn({
                let invoked = Arc::clone(&invoked);
                move |_| {
                    invoked.store(true, Ordering::Relaxed);
                    ready(42)
                }
            });
            let blocking = cx.scheduler().spawn_blocking({
                let invoked = Arc::clone(&invoked);
                move || {
                    invoked.store(true, Ordering::Relaxed);
                    42
                }
            });
            let local = local.spawn({
                let invoked = Arc::clone(&invoked);
                move || {
                    invoked.store(true, Ordering::Relaxed);
                    ready(42)
                }
            });
            let mut context = Context::from_waker(Waker::noop());
            for outcome in [
                pin!(remote).poll(&mut context),
                pin!(blocking).poll(&mut context),
                pin!(local).poll(&mut context),
            ] {
                let Poll::Ready(Err(error)) = outcome else {
                    panic!("rejected submissions must be immediately ready with an error");
                };
                assert!(error.is_shutdown());
            }
            assert!(!invoked.load(Ordering::Relaxed));
        })
        .unwrap();
}

#[test]
fn shutdown_cancels_pending_async_work_and_destroys_its_future() {
    struct Dropped(mpsc::Sender<()>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.send(()).unwrap();
        }
    }

    execute_or_terminate_process(|| {
        let runtime = runtime();
        let (started, receive_start) = mpsc::channel();
        let (dropped, receive_drop) = mpsc::channel();
        let task = runtime.task_scheduler().spawn(async move |_| {
            let guard = Dropped(dropped);
            started.send(()).unwrap();
            pending::<()>().await;
            drop(guard);
        });
        receive_start.recv_timeout(TEST_TIMEOUT).unwrap();
        runtime.stop();
        assert!(task.wait().unwrap_err().is_shutdown());
        receive_drop.recv_timeout(TEST_TIMEOUT).unwrap();
        runtime.wait();
    });
}

#[test]
fn queued_blocking_work_is_cancelled_but_running_work_finishes() {
    execute_or_terminate_process(|| {
        let runtime = runtime();
        let scheduler = runtime.task_scheduler();
        let (started, receive_start) = mpsc::channel();
        let (release, receive_release) = mpsc::channel();
        let running = scheduler.spawn_blocking(move || {
            started.send(()).unwrap();
            receive_release.recv().unwrap();
            42
        });
        receive_start.recv_timeout(TEST_TIMEOUT).unwrap();
        let invoked = Arc::new(AtomicBool::new(false));
        let queued = scheduler.spawn_blocking({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Relaxed)
        });
        runtime.stop();
        assert!(scheduler.spawn_blocking(|| 7).wait().unwrap_err().is_shutdown());
        release.send(()).unwrap();
        assert_eq!(running.wait().unwrap(), 42);
        assert!(queued.wait().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Relaxed));
        runtime.wait();
    });
}

#[test]
fn root_execution_returns_errors_and_preserves_borrowed_storage() {
    let runtime = runtime();
    assert!(runtime.block_on(async |_| panic!("root panic")).unwrap_err().is_panic());
    runtime.stop();
    let mut value = 0;
    assert!(runtime.block_on(async |_| value = 42).unwrap_err().is_shutdown());
    assert_eq!(value, 0);
    runtime.wait();
}

#[test]
fn join_error_is_a_thread_safe_error() {
    static_assertions::assert_impl_all!(JoinError: std::error::Error, Send, Sync);
}

#[test]
fn shutdown_discards_queued_async_factories_before_invocation() {
    let (queued, invoked) = runtime()
        .run(async |cx| {
            let invoked = Arc::new(AtomicBool::new(false));
            let queued = cx.scheduler().spawn({
                let invoked = Arc::clone(&invoked);
                move |_| {
                    invoked.store(true, Ordering::Relaxed);
                    ready(())
                }
            });
            cx.runtime_operations().stop();
            (queued, invoked)
        })
        .unwrap();
    assert!(queued.wait().unwrap_err().is_shutdown());
    assert!(!invoked.load(Ordering::Relaxed));
}

#[test]
fn cancelling_the_root_returns_a_shutdown_error() {
    execute_or_terminate_process(|| {
        let result = runtime().run(async |cx| {
            cx.runtime_operations().stop();
            pending::<()>().await;
        });
        assert!(result.unwrap_err().is_shutdown());
    });
}

#[cfg(feature = "macros")]
#[arty::test(workers = 1)]
#[should_panic(expected = "runtime is shutting down")]
async fn macro_boundary_reports_root_cancellation(cx: arty::runtime::Builtins) {
    cx.runtime_operations().stop();
    pending::<()>().await;
}
