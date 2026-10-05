// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Task failure and shutdown are results, not unwinds or permanently pending joins.

use std::cell::Cell;
use std::error::Error as _;
use std::future::{pending, ready};
use std::marker::PhantomPinned;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime, RuntimeOperations};
use arty::task::{JoinError, JoinHandle};
use testing_aids::{TEST_TIMEOUT, execute_or_terminate_process};
use thread_aware::Unaware;

testing_aids::init_tracing!();

#[cfg(test)]
fn runtime() -> Runtime {
    Runtime::builder()
        .processor_count(ProcessorCount::exactly(1))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
        .build()
        .unwrap()
}

#[test]
fn async_and_blocking_panics_do_not_unwind_the_joiner() {
    let runtime = runtime();
    let scheduler = runtime.scheduler();
    let handles: [JoinHandle<()>; 3] = [
        scheduler.spawn_anywhere((), |_, ()| -> std::future::Ready<()> { panic!("factory panic") }),
        scheduler.spawn_anywhere((), |_, ()| async { panic!("poll panic") }),
        scheduler.spawn_blocking(|| panic!("blocking panic")),
    ];
    for handle in handles {
        let result = catch_unwind(AssertUnwindSafe(|| handle.wait())).unwrap();
        let error = result.unwrap_err();
        assert!(error.is_panic());
        assert!(!error.is_shutdown());
    }
    assert_eq!(scheduler.spawn_anywhere((), |_, ()| async { 42 }).wait().unwrap(), 42);
}

#[test]
fn blocking_callback_cannot_wait_for_blocking_work() {
    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(1))
        .blocking_pool_policy(BlockingPoolPolicy::shared(2))
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let nested_scheduler = scheduler.clone();
    let outer = scheduler.spawn_blocking(move || {
        let mut inner = Vec::with_capacity(6);
        for _ in 0..6 {
            inner.push(nested_scheduler.spawn_blocking(|| 7));
        }
        let first = inner.into_iter().next().unwrap();
        catch_unwind(AssertUnwindSafe(|| first.wait())).is_err()
    });

    assert!(outer.wait().unwrap());
    runtime.stop().unwrap();
}

#[test]
fn shutdown_rejects_a_direct_worker_submission_before_factory_invocation() {
    let runtime = runtime();
    let invoked = Arc::new(AtomicBool::new(false));
    let factory_invoked = Arc::clone(&invoked);

    runtime
        .scheduler()
        .block_on(move |cx| async move {
            let operations = RuntimeOperations::from(&cx);
            drop(cx.scheduler().spawn(move |_| {
                factory_invoked.store(true, Ordering::Release);
                async {}
            }));
            operations.request_stop();
        })
        .unwrap();
    runtime.stop().unwrap();

    assert!(!invoked.load(Ordering::Acquire));
}

#[test]
fn local_factory_and_poll_panics_are_join_errors() {
    runtime()
        .scheduler()
        .block_on(async |cx| {
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
fn local_future_destructor_panic_preserves_runtime_usability() {
    struct ReadyDropPanic {
        dropped: Rc<Cell<bool>>,
        _pinned: PhantomPinned,
    }

    impl Future for ReadyDropPanic {
        type Output = Rc<u32>;

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(Rc::new(42))
        }
    }

    impl Drop for ReadyDropPanic {
        fn drop(&mut self) {
            self.dropped.set(true);
            panic!("local future destructor panic");
        }
    }

    execute_or_terminate_process(|| {
        let runtime = runtime();
        runtime
            .scheduler()
            .block_on(async |cx| {
                let scheduler = cx.local_scheduler().unwrap();
                let invoked = Rc::new(Cell::new(false));
                let dropped = Rc::new(Cell::new(false));
                let task = scheduler.spawn({
                    let invoked = Rc::clone(&invoked);
                    let dropped = Rc::clone(&dropped);
                    move || {
                        invoked.set(true);
                        ReadyDropPanic {
                            dropped,
                            _pinned: PhantomPinned,
                        }
                    }
                });
                assert!(invoked.get());
                let error = task.await.unwrap_err();
                assert!(error.is_panic());
                assert!(!error.is_shutdown());
                assert!(dropped.get());
                let value = scheduler.spawn(async || Rc::new(7)).await.unwrap();
                assert_eq!(*value, 7);
            })
            .unwrap();
        assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).wait().unwrap(), 42);
        runtime.stop().unwrap();
    });
}

#[test]
fn stop_rejects_remote_blocking_and_local_factories_immediately() {
    runtime()
        .scheduler()
        .block_on(async |cx| {
            let local = cx.local_scheduler().unwrap();
            RuntimeOperations::from(&cx).request_stop();
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
        let task = runtime
            .scheduler()
            .spawn_anywhere(Unaware((started, dropped)), |_, Unaware((started, dropped))| async move {
                let guard = Dropped(dropped);
                started.send(()).unwrap();
                pending::<()>().await;
                drop(guard);
            });
        receive_start.recv_timeout(TEST_TIMEOUT).unwrap();
        runtime.stop().unwrap();
        assert!(task.wait().unwrap_err().is_shutdown());
        receive_drop.recv_timeout(TEST_TIMEOUT).unwrap();
    });
}

#[test]
fn queued_blocking_work_is_cancelled_but_running_work_finishes() {
    execute_or_terminate_process(|| {
        let runtime = runtime();
        let operations = RuntimeOperations::from(&runtime);
        let scheduler = runtime.scheduler();
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
        operations.request_stop();
        assert!(scheduler.spawn_blocking(|| 7).wait().unwrap_err().is_shutdown());
        release.send(()).unwrap();
        assert_eq!(running.wait().unwrap(), 42);
        assert!(queued.wait().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Relaxed));
        runtime.stop().unwrap();
    });
}

#[test]
fn root_execution_returns_errors_and_preserves_borrowed_storage() {
    let runtime = runtime();
    assert!(
        runtime
            .scheduler()
            .block_on(async |_| panic!("root panic"))
            .unwrap_err()
            .source()
            .unwrap()
            .downcast_ref::<JoinError>()
            .unwrap()
            .is_panic()
    );
    RuntimeOperations::from(&runtime).request_stop();
    let mut value = 0;
    assert!(
        runtime
            .scheduler()
            .block_on(async |_| value = 42)
            .unwrap_err()
            .source()
            .unwrap()
            .downcast_ref::<JoinError>()
            .unwrap()
            .is_shutdown()
    );
    assert_eq!(value, 0);
    runtime.stop().unwrap();
}

#[test]
fn join_error_is_a_thread_safe_error() {
    static_assertions::assert_impl_all!(JoinError: std::error::Error, Send, Sync);
}

#[test]
fn shutdown_discards_queued_async_factories_before_invocation() {
    let (queued, invoked) = runtime()
        .scheduler()
        .block_on(async |cx| {
            let invoked = Arc::new(AtomicBool::new(false));
            let queued = cx.scheduler().spawn({
                let invoked = Arc::clone(&invoked);
                move |_| {
                    invoked.store(true, Ordering::Relaxed);
                    ready(())
                }
            });
            RuntimeOperations::from(&cx).request_stop();
            (queued, invoked)
        })
        .unwrap();
    assert!(queued.wait().unwrap_err().is_shutdown());
    assert!(!invoked.load(Ordering::Relaxed));
}

#[test]
fn cancelling_the_root_returns_a_shutdown_error() {
    execute_or_terminate_process(|| {
        let result = runtime().scheduler().block_on(async |cx| {
            RuntimeOperations::from(&cx).request_stop();
            pending::<()>().await;
        });
        assert!(
            result
                .unwrap_err()
                .source()
                .unwrap()
                .downcast_ref::<JoinError>()
                .unwrap()
                .is_shutdown()
        );
    });
}

#[cfg(feature = "macros")]
#[arty::test(workers = 1)]
#[should_panic(expected = "runtime is shutting down")]
async fn macro_boundary_reports_root_cancellation(cx: arty::task::Builtins) {
    RuntimeOperations::from(&cx).request_stop();
    pending::<()>().await;
}
