// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scheduling, wake, lifetime, clock, and shutdown compatibility contracts.

#![cfg(feature = "rt")]

use std::cell::Cell;
use std::future::{pending, poll_fn};
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};
use testing_aids::{TEST_TIMEOUT, execute_or_terminate_process};
use thread_aware::ThreadAware;
use tick::{ClockControl, FutureExt};

testing_aids::init_tracing!();

#[cfg(test)]
fn runtime(workers: usize) -> Runtime {
    Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::new(workers).unwrap()))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
        .build()
        .unwrap()
}

#[test]
fn detached_handles_share_round_robin_but_bound_handles_retain_affinity() {
    execute_or_terminate_process(|| {
        let runtime = runtime(2);
        let first = runtime.task_scheduler();
        let clone = first.clone();
        let another = runtime.task_scheduler();
        let (a, bound) = first
            .spawn(async |cx| (thread::current().id(), cx.scheduler().clone()))
            .wait()
            .unwrap();
        let b = clone.spawn(async |_| thread::current().id()).wait().unwrap();
        let a_again = another.spawn(async |_| thread::current().id()).wait().unwrap();
        assert_ne!(a, b);
        assert_eq!(a, a_again);
        let bound_clone = bound.clone();
        let cloned_result = thread::spawn(move || bound_clone.spawn(async |_| thread::current().id()).wait().unwrap())
            .join()
            .unwrap();
        assert_eq!(cloned_result, a);
        assert_eq!(bound.spawn(async |_| thread::current().id()).wait().unwrap(), a);
        assert_eq!(first.spawn(async |_| thread::current().id()).wait().unwrap(), b);
    });
}

#[test]
fn detached_relocation_binds_only_the_notified_clone() {
    let runtime = runtime(2);
    let detached = runtime.task_scheduler();
    let home = detached.spawn(async |cx| cx.thread().clone()).wait().unwrap();
    let mut bound = detached.clone();
    bound.relocate(None, &home);
    for _ in 0..4 {
        assert_eq!(bound.spawn(async |_| thread::current().id()).wait().unwrap(), home.id());
    }
    assert_ne!(detached.spawn(async |_| thread::current().id()).wait().unwrap(), home.id());
}

#[test]
fn concurrent_schedulers_share_selection_without_losing_submissions() {
    execute_or_terminate_process(|| {
        let tasks_per_producer = if cfg!(miri) { 2 } else { 25 };
        let runtime = runtime(2);
        let scheduler = runtime.task_scheduler();
        let results = thread::scope(|scope| {
            let producers = std::array::from_fn::<_, 4, _>(|_| {
                let scheduler = scheduler.clone();
                scope.spawn(move || {
                    (0..tasks_per_producer)
                        .map(|_| scheduler.spawn(async |_| thread::current().id()).wait().unwrap())
                        .collect::<Vec<_>>()
                })
            });
            producers
                .into_iter()
                .flat_map(|producer| producer.join().unwrap())
                .collect::<Vec<_>>()
        });
        let first = results[0];
        assert_eq!(results.len(), 4 * tasks_per_producer);
        assert_eq!(results.iter().filter(|id| **id == first).count(), 2 * tasks_per_producer);
        assert_eq!(results.into_iter().collect::<std::collections::HashSet<_>>().len(), 2);
    });
}

#[test]
fn closed_runtime_rejects_factories_with_an_immediate_shutdown_error() {
    let runtime = runtime(1);
    let scheduler = runtime.task_scheduler();
    drop(runtime);
    let invoked = Arc::new(AtomicBool::new(false));
    let captured = Arc::clone(&invoked);
    let mut join = Box::pin(scheduler.spawn(move |_: arty::runtime::Builtins| {
        captured.store(true, Ordering::Relaxed);
        async {}
    }));
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Err(error)) = join.as_mut().poll(&mut cx) else {
        panic!("a rejected join must be ready with a shutdown error");
    };
    assert!(error.is_shutdown());
    assert!(!invoked.load(Ordering::Relaxed));
}

#[test]
fn blocking_tasks_leave_the_async_worker_responsive() {
    execute_or_terminate_process(|| {
        let runtime = runtime(1);
        let (started, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let blocking = runtime.task_scheduler().spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            42
        });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(runtime.task_scheduler().spawn(async |_| 17).wait().unwrap(), 17);
        release.send(()).unwrap();
        assert_eq!(blocking.wait().unwrap(), 42);
    });
}

#[test]
fn pending_future_is_woken_from_an_unrelated_thread() {
    execute_or_terminate_process(|| {
        let runtime = runtime(1);
        let (send, receive) = mpsc::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let ready_task = Arc::clone(&ready);
        let task = runtime.task_scheduler().spawn(async move |_| {
            let mut send = Some(send);
            poll_fn(move |cx| {
                if ready_task.load(Ordering::Acquire) {
                    Poll::Ready(42)
                } else {
                    if let Some(send) = send.take() {
                        send.send(cx.waker().clone()).unwrap();
                    }
                    Poll::Pending
                }
            })
            .await
        });
        let waker = receive.recv_timeout(TEST_TIMEOUT).unwrap();
        thread::spawn(move || {
            ready.store(true, Ordering::Release);
            waker.wake();
        })
        .join()
        .unwrap();
        assert_eq!(task.wait().unwrap(), 42);
    });
}

#[test]
fn worker_can_drive_a_controlled_clock_without_io() {
    execute_or_terminate_process(|| {
        let control = ClockControl::new().auto_advance_timers(true);
        let runtime = Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
            .clock(control)
            .build()
            .unwrap();
        runtime
            .run(async |cx| {
                let watch = cx.clock().stopwatch();
                cx.clock().delay(Duration::from_secs(5)).await;
                assert!(watch.elapsed() >= Duration::from_secs(5));
                assert!(pending::<()>().timeout(cx.clock(), Duration::from_secs(1)).await.is_err());
            })
            .unwrap();
    });
}

#[test]
fn factory_and_poll_panics_return_errors_and_preserve_runtime_usability() {
    #[derive(Debug, PartialEq)]
    struct Payload(u32);

    fn panic_factory(_: arty::runtime::Builtins) -> std::future::Ready<()> {
        panic_any(Payload(1))
    }
    let runtime = runtime(1);
    let factory = runtime.task_scheduler().spawn(panic_factory);
    let polling = runtime.task_scheduler().spawn(async |_| panic_any(Payload(2)));
    for task in [factory, polling] {
        let outcome = catch_unwind(AssertUnwindSafe(|| task.wait())).unwrap();
        assert!(outcome.unwrap_err().is_panic());
    }
    assert_eq!(runtime.task_scheduler().spawn(async |_| 42).wait().unwrap(), 42);
}

#[test]
fn scoped_borrowed_storage_is_destroyed_before_success_or_error() {
    struct Borrowed<'a>(&'a AtomicBool);
    impl Drop for Borrowed<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    let runtime = runtime(1);
    for should_panic in [false, true] {
        let destroyed = AtomicBool::new(false);
        let borrowed = Borrowed(&destroyed);
        let result = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(async move |_| {
                let _borrowed = borrowed;
                if should_panic {
                    panic_any(17u32);
                }
                42
            })
        }))
        .unwrap();
        assert!(destroyed.load(Ordering::Acquire));
        match result {
            Ok(value) => assert_eq!((should_panic, value), (false, 42)),
            Err(error) => assert!(should_panic && error.is_panic()),
        }
    }
}

#[test]
fn local_non_send_state_is_destroyed_on_its_worker() {
    struct LocalDrop {
        owner: thread::ThreadId,
        dropped: Rc<Cell<bool>>,
    }

    impl Drop for LocalDrop {
        fn drop(&mut self) {
            assert_eq!(thread::current().id(), self.owner);
            self.dropped.set(true);
        }
    }

    let runtime = runtime(1);
    let portable = runtime
        .task_scheduler()
        .spawn(async |cx| {
            let dropped = Rc::new(Cell::new(false));
            let value = LocalDrop {
                owner: thread::current().id(),
                dropped: Rc::clone(&dropped),
            };
            cx.local_scheduler().unwrap().spawn(async move || drop(value)).await.unwrap();
            assert!(dropped.get());
            cx.clone()
        })
        .wait()
        .unwrap();
    thread::spawn({
        let portable = portable.clone();
        move || {
            assert!(portable.local_scheduler().is_none());
            drop(portable);
        }
    })
    .join()
    .unwrap();
    drop(runtime);
    thread::spawn(move || drop(portable)).join().unwrap();
}

#[test]
fn cancellation_cleanup_cannot_reenter_the_local_executor() {
    struct Cleanup {
        scheduler: arty::task::LocalTaskScheduler,
        dropped: mpsc::Sender<bool>,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let invoked = Rc::new(Cell::new(false));
            let captured = Rc::clone(&invoked);
            let handle = self.scheduler.spawn(move || {
                captured.set(true);
                async {}
            });
            drop(handle);
            self.dropped.send(invoked.get()).unwrap();
        }
    }

    execute_or_terminate_process(|| {
        let runtime = runtime(1);
        let (started, start) = mpsc::channel();
        let (dropped, drop_result) = mpsc::channel();
        let handle = runtime.task_scheduler().spawn(async move |cx| {
            let cleanup = Cleanup {
                scheduler: cx.local_scheduler().unwrap(),
                dropped,
            };
            started.send(()).unwrap();
            pending::<()>().await;
            drop(cleanup);
        });
        start.recv_timeout(TEST_TIMEOUT).unwrap();
        runtime.stop();
        runtime.wait();
        assert!(!drop_result.recv_timeout(TEST_TIMEOUT).unwrap());
        drop(handle);
    });
}

#[test]
fn a_blocking_task_can_drop_its_runtime_without_joining_itself() {
    execute_or_terminate_process(|| {
        for policy in [BlockingPoolPolicy::isolated(), BlockingPoolPolicy::shared(1)] {
            let runtime = Runtime::builder()
                .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
                .blocking_pool_policy(policy)
                .build()
                .unwrap();
            let scheduler = runtime.task_scheduler();
            let (finished, receive) = mpsc::channel();
            let task = scheduler.spawn_blocking(move || {
                drop(runtime);
                finished.send(()).unwrap();
            });
            receive.recv_timeout(TEST_TIMEOUT).unwrap();
            task.wait().unwrap();
        }
    });
}

#[test]
fn repeated_waits_report_one_completed_shutdown() {
    let (sink, processor) = observed_testing::test_emitter(observed_testing::TEST_ID);
    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .sink(sink)
        .build()
        .unwrap();
    runtime.stop();
    runtime.wait();
    runtime.wait();
    drop(runtime);
    assert_eq!(
        processor.events().iter().filter(|event| event.name() == "arty.rt.stopped").count(),
        1
    );
}
