// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public construction, context, ownership, service, and join contracts.

#![cfg(feature = "rt")]
#![cfg(test)]

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;

use arty::runtime::{BlockingPoolPolicy, CpuPolicy, Runtime, RuntimeOperations};
use arty::task::{Builtins, Scheduler};
use arty::time::Clock;
use futures::future::join_all;
use observed::Sink;
use observed_testing::{TEST_ID, test_emitter};
use testing_aids::{TEST_TIMEOUT, execute_or_terminate_process};
use thread_aware::Unaware;

testing_aids::init_tracing!();

fn runtime(workers: usize) -> Runtime {
    Runtime::builder().cpu_policy(CpuPolicy::exactly(workers)).build().unwrap()
}

fn worker_scheduler(runtime: &Runtime) -> Scheduler {
    runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap()
}

fn count_workers(runtime: &Runtime) -> usize {
    let tasks = worker_scheduler(runtime).spawn_everywhere((), |()| async { thread::current().id() });
    let count = tasks.len();
    let workers: std::collections::HashSet<_> = tasks.into_iter().map(|task| task.wait().unwrap()).collect();
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

#[test]
fn public_policy_defaults_are_automatic_and_shared() {
    const AUTOMATIC: CpuPolicy = CpuPolicy::auto();
    assert_eq!(CpuPolicy::default(), AUTOMATIC);
    assert_eq!(BlockingPoolPolicy::default(), BlockingPoolPolicy::shared(None));
}

#[test]
fn the_last_processor_setting_replaces_invalid_and_previous_counts() {
    for previous in [
        CpuPolicy::exactly(0),
        CpuPolicy::at_most(0),
        CpuPolicy::exactly(usize::MAX),
        CpuPolicy::exactly(2),
        CpuPolicy::all(),
        CpuPolicy::auto(),
    ] {
        let runtime = Runtime::builder()
            .cpu_policy(previous)
            .cpu_policy(CpuPolicy::at_most(1))
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
fn resource_setters_preserve_worker_count_stack_and_pool_independently() {
    const STACK: usize = 4 * 1024 * 1024;
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder()
        .stack_size(STACK)
        .blocking_pool_policy(BlockingPoolPolicy::isolated())
        .cpu_policy(CpuPolicy::at_most(1))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
        .cpu_policy(CpuPolicy::exactly(2))
        .sink(sink)
        .build()
        .unwrap();
    assert_eq!(count_workers(&runtime), 2);
    runtime
        .scheduler()
        .block_on(async |cx| {
            assert!(!cx.sink().is_noop());
            cx.sink().flush().unwrap();
        })
        .unwrap();
    let first = worker_scheduler(&runtime);
    let second = worker_scheduler(&runtime);
    assert_eq!(
        first.spawn_blocking(|| thread::current().id()).wait().unwrap(),
        second.spawn_blocking(|| thread::current().id()).wait().unwrap()
    );
    runtime.stop().unwrap();
    let events = processor.events();
    let started: Vec<_> = events.iter().filter(|event| event.name() == "arty.rt.started").collect();
    assert_eq!(started.len(), 1);
    let configured_minimum = std::env::var("RUST_MIN_STACK")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let dimensions = started[0].dimensions();
    assert!(dimensions.contains(&("processors.used".into(), "2".into())));
    assert!(dimensions.contains(&("blocking_worker_pool.mode".into(), "shared".into())));
    assert!(dimensions.contains(&("stack_size_bytes".into(), STACK.max(configured_minimum).to_string().into())));
}

#[test]
fn construction_and_shutdown_work_inside_a_futures_executor() {
    execute_or_terminate_process(|| {
        futures::executor::block_on(async {
            let runtime = runtime(1);
            let (answer, scheduler) = runtime
                .scheduler()
                .spawn_anywhere((), |cx, ()| async move {
                    assert!(cx.local_scheduler().is_some());
                    assert_eq!(cx.thread().id(), thread::current().id());
                    (42, cx.scheduler().clone())
                })
                .await
                .unwrap();
            assert_eq!(answer, 42);
            runtime.stop().unwrap();
            assert!(scheduler.spawn(async |_| ()).await.unwrap_err().is_shutdown());
        });
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "self-stop intentionally returns before the worker can finish; native and careful suites retain the guard contract"
)]
fn explicit_stop_on_an_async_worker_returns_an_error_without_unwinding() {
    completes_without_deadlock(|| {
        let runtime = runtime(1);
        let scheduler = worker_scheduler(&runtime);
        assert!(
            scheduler
                .spawn(async |cx| cx.thread().id() == thread::current().id())
                .wait()
                .unwrap()
        );
        let outcome = scheduler.spawn(async move |_| runtime.stop()).wait().unwrap();
        assert!(outcome.unwrap_err().to_string().contains("async Arty worker"));
    });
}

#[test]
fn explicit_stop_in_its_blocking_callback_returns_an_error_without_self_joining() {
    let runtime = runtime(1);
    let scheduler = worker_scheduler(&runtime);
    assert!(
        scheduler.spawn_blocking(|| thread::current().id()).wait().unwrap()
            != scheduler.spawn(async |_| thread::current().id()).wait().unwrap()
    );
    let outcome = scheduler.spawn_blocking(move || runtime.stop()).wait().unwrap();
    assert!(outcome.unwrap_err().to_string().contains("blocking callback"));
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
fn worker_context_belongs_to_one_runtime_and_rejects_all_nested_waits() {
    completes_without_deadlock(|| {
        let first = Runtime::builder()
            .cpu_policy(CpuPolicy::exactly(1))
            .blocking_pool_policy(BlockingPoolPolicy::shared(1))
            .build()
            .unwrap();
        let second = runtime(1);
        let first_scheduler = first.scheduler();
        let second_scheduler = second.scheduler();
        assert!(!first_scheduler.is_on_worker());
        assert!(!second_scheduler.is_on_worker());
        let ready = first_scheduler.spawn_blocking(|| 42);
        first_scheduler.spawn_blocking(|| ()).wait().unwrap();
        first_scheduler
            .block_on(async |_| {
                assert!(first_scheduler.is_on_worker());
                assert!(!second_scheduler.is_on_worker());
                // The single blocking pool has already delivered this result.
                // Prove the worker wait guard before attempting a nested borrowing wait.
                catch_unwind(AssertUnwindSafe(|| ready.wait())).unwrap_err();
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
fn blocking_callbacks_can_run_borrowing_tasks_on_an_async_worker() {
    execute_or_terminate_process(|| {
        let runtime = Arc::new(runtime(1));
        let captured = Arc::clone(&runtime);
        let (on_worker, value) = runtime
            .scheduler()
            .spawn_blocking(move || {
                assert!(!captured.scheduler().is_on_worker());
                let mut value = 40;
                let on_worker = captured
                    .scheduler()
                    .block_on(async |cx| {
                        let child = cx.scheduler().spawn(async |_| 2).await.unwrap();
                        let local = Rc::new(child);
                        value += *local;
                        captured.scheduler().is_on_worker()
                    })
                    .unwrap();
                (on_worker, value)
            })
            .wait()
            .unwrap();
        assert_eq!((on_worker, value), (true, 42));
        Arc::try_unwrap(runtime).unwrap().stop().unwrap();
    });
}

#[test]
fn builtins_publish_the_same_services_on_every_worker() {
    let runtime = runtime(2);
    let builtins = runtime.scheduler().block_on(async |cx| cx).unwrap();
    let tasks = builtins.scheduler().spawn_everywhere(builtins.clone(), |cx: Builtins| async move {
        let scheduler: &Scheduler = cx.as_ref();
        let clock: &Clock = cx.as_ref();
        let sink: &Sink = cx.as_ref();
        assert!(std::ptr::eq(scheduler, cx.scheduler()));
        assert!(std::ptr::eq(clock, cx.clock()));
        assert!(std::ptr::eq(sink, cx.sink()));
        assert!(sink.is_noop());
        assert!(cx.local_scheduler().is_some());
        assert_eq!(cx.thread().id(), thread::current().id());
        assert_eq!(scheduler.spawn(async |_| 42).await.unwrap(), 42);
        cx.thread().id()
    });
    assert_eq!(tasks.len(), 2);
    let workers: std::collections::HashSet<_> = tasks.into_iter().map(|task| task.wait().unwrap()).collect();
    assert_eq!(workers.len(), 2);
    assert!(builtins.local_scheduler().is_none());
    runtime.stop().unwrap();
}

#[cfg(not(miri))]
#[test]
fn worker_services_are_ready_before_spawning() {
    let count = many_cpus::SystemHardware::current().processors().len().min(2);
    let runtime = Runtime::builder().cpu_policy(CpuPolicy::at_most(2)).build().unwrap();
    let (actual, expected) = runtime
        .scheduler()
        .block_on(async move |cx| {
            let scheduler = cx.scheduler();
            let tasks: Vec<_> = (0..count)
                .map(|_| {
                    scheduler.spawn_anywhere(cx.clone(), |worker: Builtins| async move {
                        (worker.thread().id() == thread::current().id(), worker.local_scheduler().is_some())
                    })
                })
                .collect();
            let expected = vec![(true, true); tasks.len()];
            (join_all(tasks).await.into_iter().map(Result::unwrap).collect::<Vec<_>>(), expected)
        })
        .unwrap();

    assert_eq!(actual, expected);
}

#[cfg(not(miri))]
#[test]
fn emitter_is_available_by_default() {
    let runtime = Runtime::builder().build().unwrap();

    runtime
        .scheduler()
        .block_on(async move |cx: Builtins| {
            assert!(cx.sink().is_noop());
        })
        .unwrap();
}

#[cfg(not(miri))]
#[test]
fn configured_emitter_is_available_from_builtins() {
    use observed::metadata::EventDescription;
    use observed::processing::{EventProcessor, EventView};

    struct TestProcessor;

    impl EventProcessor for TestProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            true
        }

        fn process(&self, _event: &EventView<'_>) {}

        fn flush(&self) -> Result<(), observed::FlushError> {
            Ok(())
        }
    }

    let sink = Sink::new("test", vec![Arc::new(TestProcessor)], tick::SimpleClock::new_frozen());
    let runtime = Runtime::builder().sink(sink).build().unwrap();

    runtime
        .scheduler()
        .block_on(async move |cx: Builtins| {
            assert!(!cx.sink().is_noop());
            cx.sink().flush().unwrap();
        })
        .unwrap();
}

#[cfg(feature = "test-util")]
#[test]
fn configured_clock_control_remains_the_worker_time_source() {
    use std::time::Duration;

    use arty::time::ClockControl;

    let control = ClockControl::new();
    let runtime = Runtime::builder()
        .cpu_policy(CpuPolicy::exactly(1))
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
        .map(|scheduler| scheduler.spawn(async |cx| cx.thread().id()).wait().unwrap())
        .collect();
    assert_ne!(workers[0], workers[1]);
    let blocking: Vec<_> = schedulers
        .iter()
        .map(|scheduler| scheduler.spawn_blocking(|| thread::current().id()).wait().unwrap())
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
    let first_worker = first_scheduler.spawn(async |cx| cx.thread().id()).wait().unwrap();
    let second_worker = second_scheduler.spawn(async |cx| cx.thread().id()).wait().unwrap();
    assert_ne!(first_worker, second_worker);
    RuntimeOperations::from(&first).request_stop();
    first.stop().unwrap();
    assert!(first_scheduler.spawn(async |_| 42).wait().unwrap_err().is_shutdown());
    assert_eq!(second_scheduler.spawn(async |_| 42).wait().unwrap(), 42);
    second.stop().unwrap();
}

#[test]
fn dropping_another_runtime_from_a_worker_does_not_wait_for_its_blocking_callback() {
    execute_or_terminate_process(|| {
        let target = runtime(1);
        let caller = runtime(1);
        let retained = worker_scheduler(&target);
        let (started, ready) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let callback = target.scheduler().spawn_blocking(move || {
            started.send(()).unwrap();
            gate.recv_timeout(TEST_TIMEOUT).unwrap();
            42
        });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        caller
            .scheduler()
            .spawn_anywhere(Unaware(target), |_, Unaware(target)| async move { drop(target) })
            .wait()
            .unwrap();
        assert!(retained.spawn(async |_| 42).wait().unwrap_err().is_shutdown());
        release.send(()).unwrap();
        assert_eq!(callback.wait().unwrap(), 42);
        caller.stop().unwrap();
    });
}

#[test]
fn application_errors_remain_results_for_async_local_and_blocking_tasks() {
    let runtime = runtime(1);
    assert_eq!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { Err::<(), _>("application") })
            .wait()
            .unwrap(),
        Err("application")
    );
    assert_eq!(
        runtime.scheduler().spawn_blocking(|| Err::<(), _>("application")).wait().unwrap(),
        Err("application")
    );
    assert_eq!(
        runtime
            .scheduler()
            .block_on(async |cx| cx
                .local_scheduler()
                .unwrap()
                .spawn(async || Err::<(), _>("application"))
                .await
                .unwrap())
            .unwrap(),
        Err("application")
    );
    runtime.stop().unwrap();
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
    scheduler.spawn(async |_| ()).wait().unwrap();
    let Poll::Ready(Ok(value)) = task.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("an already-completed task must be immediately ready");
    };
    assert_eq!(value.get(), 42);
    catch_unwind(AssertUnwindSafe(|| task.as_mut().poll(&mut Context::from_waker(Waker::noop())))).unwrap_err();
    assert_eq!(
        runtime.scheduler().spawn_anywhere((), |_, ()| async { 123u32 }).wait().unwrap(),
        123
    );
    runtime.stop().unwrap();
}

#[test]
fn rejected_remote_joins_report_shutdown_then_reject_repolling() {
    let runtime = runtime(1);
    RuntimeOperations::from(&runtime).request_stop();
    let mut task = Box::pin(runtime.scheduler().spawn_anywhere((), |_, ()| async {}));
    let Poll::Ready(Err(error)) = task.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("a rejected submission must be immediately ready");
    };
    assert!(error.is_shutdown());
    assert!(!error.is_panic());
    catch_unwind(AssertUnwindSafe(|| task.as_mut().poll(&mut Context::from_waker(Waker::noop())))).unwrap_err();
    runtime.stop().unwrap();
}

#[test]
fn local_join_keeps_non_send_results_and_rejects_repolling() {
    let runtime = runtime(1);
    runtime
        .scheduler()
        .block_on(async |cx| {
            let result = Rc::new(42);
            let captured = Rc::clone(&result);
            let mut task = Box::pin(cx.local_scheduler().unwrap().spawn(async move || captured));
            let returned = task.as_mut().await.unwrap();
            assert!(Rc::ptr_eq(&returned, &result));
            catch_unwind(AssertUnwindSafe(|| task.as_mut().poll(&mut Context::from_waker(Waker::noop())))).unwrap_err();
        })
        .unwrap();
    runtime.stop().unwrap();
}

#[cfg(feature = "macros")]
mod entry_points {
    use arty::task::Builtins;

    #[arty::main(workers = 1)]
    async fn application_error(_cx: Builtins) -> Result<(), &'static str> {
        Err("application")
    }

    #[test]
    fn main_preserves_application_errors_as_return_values() {
        assert_eq!(application_error(), Err("application"));
    }
}
