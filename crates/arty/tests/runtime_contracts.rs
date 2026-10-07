// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scheduling, wake, lifetime, clock, and shutdown compatibility contracts.

#![cfg(feature = "rt")]

use std::cell::Cell;
use std::error::Error as _;
use std::future::{pending, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use arty::runtime::{BlockingPoolPolicy, Runtime, RuntimeOperations, WorkersPolicy};
#[cfg(all(feature = "macros", feature = "test-util"))]
use arty::task::Builtins;
use arty::task::JoinError;
use testing_aids::TEST_TIMEOUT;
use thread_aware::{ThreadAware, Unaware};
#[cfg(all(feature = "macros", feature = "test-util"))]
use tick::ClockControl;
#[cfg(all(feature = "macros", feature = "test-util"))]
use tick::FutureExt;

testing_aids::init_tracing!();

#[cfg(test)]
fn runtime(workers: usize) -> Runtime {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(workers))
        .blocking_pool(BlockingPoolPolicy::shared(1))
        .build()
        .unwrap()
}

#[test]
fn borrowed_runtime_scheduler_shares_round_robin_but_bound_handles_retain_affinity() {
    let runtime = runtime(2);
    let first = runtime.scheduler();
    let another = runtime.scheduler();
    assert!(std::ptr::eq(first, another));
    let (a, bound) = first
        .spawn_anywhere((), |cx, ()| async move { (thread::current().id(), cx.scheduler().clone()) })
        .wait()
        .unwrap();
    let b = another.spawn_anywhere((), |_, ()| async { thread::current().id() }).wait().unwrap();
    let a_again = runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async { thread::current().id() })
        .wait()
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(a, a_again);
    let bound_clone = bound.clone();
    let cloned_result = thread::spawn(move || bound_clone.spawn(async |_| thread::current().id()).wait().unwrap())
        .join()
        .unwrap();
    assert_eq!(cloned_result, a);
    assert_eq!(bound.spawn(async |_| thread::current().id()).wait().unwrap(), a);
    assert_eq!(
        first.spawn_anywhere((), |_, ()| async { thread::current().id() }).wait().unwrap(),
        b
    );
}

#[test]
fn relocation_rebinds_only_the_notified_worker_scheduler_clone() {
    let runtime = runtime(2);
    let source = runtime.scheduler().spawn_anywhere((), |cx, ()| async move { cx }).wait().unwrap();
    let destination = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.thread().clone() })
        .wait()
        .unwrap();
    let original = source.scheduler().clone();
    let mut bound = original.clone();
    bound.relocate(None, &destination);
    for _ in 0..4 {
        assert_eq!(bound.spawn(async |_| thread::current().id()).wait().unwrap(), destination.id());
    }
    assert_eq!(
        original.spawn(async |_| thread::current().id()).wait().unwrap(),
        source.thread().id()
    );
    assert_ne!(source.thread(), &destination);
}

#[test]
fn concurrent_schedulers_share_selection_without_losing_submissions() {
    let tasks_per_producer = if cfg!(miri) { 2 } else { 25 };
    let runtime = runtime(2);
    let scheduler = runtime.scheduler();
    let results = thread::scope(|scope| {
        let producers = std::array::from_fn::<_, 4, _>(|_| {
            scope.spawn(move || {
                (0..tasks_per_producer)
                    .map(|_| {
                        scheduler
                            .spawn_anywhere((), |_, ()| async { thread::current().id() })
                            .wait()
                            .unwrap()
                    })
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
}

#[test]
fn registered_work_and_timers_progress_while_producers_keep_submitting() {
    struct InFlight(Arc<AtomicUsize>);

    impl Drop for InFlight {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Release);
        }
    }

    let producer_count = if cfg!(miri) { 2 } else { 4 };
    let capacity = if cfg!(miri) { 4 } else { 1024 };
    let runtime = runtime(1);
    let running = Arc::new(AtomicBool::new(true));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let submissions = Arc::new(AtomicUsize::new(0));
    let (task_go, task_gate) = events_once::Event::boxed();
    let (timer_go, timer_gate) = events_once::Event::boxed();
    let (finished, observed) = mpsc::channel();
    let task_finished = finished.clone();
    let task_running = Arc::clone(&running);
    let timer_running = Arc::clone(&running);
    let task = runtime.scheduler().spawn_anywhere(
        Unaware((task_gate, task_finished, task_running)),
        |_, Unaware((task_gate, task_finished, task_running))| async move {
            task_gate.await.unwrap();
            task_finished.send(task_running.load(Ordering::Acquire)).unwrap();
        },
    );
    let timer = runtime.scheduler().spawn_anywhere(
        Unaware((timer_gate, finished, timer_running)),
        |cx, Unaware((timer_gate, finished, timer_running))| async move {
            timer_gate.await.unwrap();
            cx.clock().delay(Duration::from_millis(1)).await;
            finished.send(timer_running.load(Ordering::Acquire)).unwrap();
        },
    );

    thread::scope(|scope| {
        let _stop = scopeguard::guard(Arc::clone(&running), |running| running.store(false, Ordering::Release));
        let (started, producers_ready) = mpsc::channel();
        for _ in 0..producer_count {
            let scheduler = runtime.scheduler();
            let running = Arc::clone(&running);
            let in_flight = Arc::clone(&in_flight);
            let submissions = Arc::clone(&submissions);
            let started = started.clone();
            scope.spawn(move || {
                started.send(()).unwrap();
                while running.load(Ordering::Acquire) {
                    if in_flight
                        .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |count| (count < capacity).then_some(count + 1))
                        .is_err()
                    {
                        thread::yield_now();
                        continue;
                    }
                    let submission = InFlight(Arc::clone(&in_flight));
                    drop(scheduler.spawn_anywhere(Unaware(submission), |_, Unaware(submission)| async move { drop(submission) }));
                    submissions.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        for _ in 0..producer_count {
            producers_ready.recv_timeout(TEST_TIMEOUT).unwrap();
        }
        while submissions.load(Ordering::Acquire) < producer_count {
            thread::yield_now();
        }
        task_go.send(());
        timer_go.send(());
        assert!(observed.recv_timeout(TEST_TIMEOUT).unwrap());
        assert!(observed.recv_timeout(TEST_TIMEOUT).unwrap());
    });

    task.wait().unwrap();
    timer.wait().unwrap();
    runtime.stop().unwrap();
    assert_eq!(in_flight.load(Ordering::Acquire), 0);
}

#[test]
fn closed_runtime_rejects_factories_with_an_immediate_shutdown_error() {
    let runtime = runtime(1);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .wait()
        .unwrap();
    runtime.stop().unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    let captured = Arc::clone(&invoked);
    let mut join = Box::pin(scheduler.spawn(move |_: arty::task::Builtins| {
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
    let runtime = runtime(1);
    let (started, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let blocking = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        wait.recv().unwrap();
        42
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 17 }).wait().unwrap(), 17);
    release.send(()).unwrap();
    assert_eq!(blocking.wait().unwrap(), 42);
}

#[test]
fn pending_future_is_woken_from_an_unrelated_thread() {
    let runtime = runtime(1);
    let (send, receive) = mpsc::channel();
    let ready = Arc::new(AtomicBool::new(false));
    let ready_task = Arc::clone(&ready);
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((send, ready_task)), |_, Unaware((send, ready_task))| async move {
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
}

#[cfg(all(feature = "macros", feature = "test-util"))]
#[arty::test]
async fn worker_can_drive_a_controlled_clock_without_io(cx: Builtins, control: ClockControl) {
    let _control = control.auto_advance_timers(true);
    let watch = cx.clock().stopwatch();
    cx.clock().delay(Duration::from_secs(5)).await;
    assert!(watch.elapsed() >= Duration::from_secs(5));
    assert!(pending::<()>().timeout(cx.clock(), Duration::from_secs(1)).await.is_err());
}

#[test]
fn factory_and_poll_panics_return_errors_and_preserve_runtime_usability() {
    #[derive(Debug, PartialEq)]
    struct Payload(u32);

    fn panic_factory(_: arty::task::Builtins) -> std::future::Ready<()> {
        panic_any(Payload(1))
    }
    let runtime = runtime(1);
    let factory = runtime.scheduler().spawn_anywhere((), |cx, ()| panic_factory(cx));
    let polling = runtime.scheduler().spawn_anywhere((), |_, ()| async { panic_any(Payload(2)) });
    for task in [factory, polling] {
        let outcome = catch_unwind(AssertUnwindSafe(|| task.wait())).unwrap();
        assert!(outcome.unwrap_err().is_panic());
    }
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).wait().unwrap(), 42);
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
            runtime.scheduler().block_on(async move |_| {
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
            Err(error) => assert!(should_panic && error.source().unwrap().downcast_ref::<JoinError>().unwrap().is_panic()),
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
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move {
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
        scheduler: arty::task::LocalScheduler,
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

    let runtime = runtime(1);
    let (started, start) = mpsc::channel();
    let (dropped, drop_result) = mpsc::channel();
    let handle = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, dropped)), |cx, Unaware((started, dropped))| async move {
            let cleanup = Cleanup {
                scheduler: cx.local_scheduler().unwrap(),
                dropped,
            };
            started.send(()).unwrap();
            pending::<()>().await;
            drop(cleanup);
        });
    start.recv_timeout(TEST_TIMEOUT).unwrap();
    runtime.stop().unwrap();
    assert!(!drop_result.recv_timeout(TEST_TIMEOUT).unwrap());
    drop(handle);
}

#[test]
fn a_blocking_task_can_drop_its_runtime_without_joining_itself() {
    for policy in [BlockingPoolPolicy::isolated(), BlockingPoolPolicy::shared(1)] {
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .blocking_pool(policy)
            .build()
            .unwrap();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
            .wait()
            .unwrap();
        let (finished, receive) = mpsc::channel();
        let task = scheduler.spawn_blocking(move || {
            drop(runtime);
            finished.send(()).unwrap();
        });
        receive.recv_timeout(TEST_TIMEOUT).unwrap();
        task.wait().unwrap();
    }
}

#[test]
fn repeated_stop_requests_report_one_completed_shutdown() {
    let (sink, processor) = observed_testing::test_emitter(observed_testing::TEST_ID);
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).sink(sink).build().unwrap();
    let operations = RuntimeOperations::from(&runtime);
    operations.request_stop();
    operations.request_stop();
    runtime.stop().unwrap();
    assert_eq!(
        processor.events().iter().filter(|event| event.name() == "arty.rt.stopped").count(),
        1
    );
}
