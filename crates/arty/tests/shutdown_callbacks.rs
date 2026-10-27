// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shutdown callbacks run after runtime work ends.

#![cfg(feature = "rt")]
#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::cell::RefCell;
use std::future::pending;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::Waker;

use arty::core::{Thread, ThreadAware};
use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
use arty::task::{Builtins, JoinError, JoinHandle};
use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, IoContext};
use performables::sync::channel::{OneshotSender, oneshot};
use testing_aids::{TEST_TIMEOUT, execute_or_abandon};

trait JoinHandleExt {
    type Output;

    fn join(self) -> Result<Self::Output, JoinError>;
}

impl<R> JoinHandleExt for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn join(self) -> Result<Self::Output, JoinError> {
        futures::executor::block_on(self)
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct DriverState(Arc<AtomicBool>);

impl ThreadAware for DriverState {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[derive(Clone)]
struct ShutdownContext(DriverState);

impl ThreadAware for ShutdownContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for ShutdownContext {
    type ContextOptions = DriverState;
    type Driver = ShutdownDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self(options)
    }

    fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
        Ok(ShutdownDriver(self.0.clone()))
    }
}

struct ShutdownDriver(DriverState);

impl Driver for ShutdownDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        self.0.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

thread_local! {
    static THREAD_LOCAL_RUNTIME: RefCell<Option<ThreadLocalRuntime>> = const { RefCell::new(None) };
}

struct ThreadLocalRuntime {
    runtime: Option<Runtime>,
    dropped: mpsc::SyncSender<()>,
}

impl Drop for ThreadLocalRuntime {
    fn drop(&mut self) {
        drop(self.runtime.take());
        self.dropped.send(()).unwrap();
    }
}

struct BlockingSetup {
    cx: Builtins,
    started: OneshotSender<()>,
    release: mpsc::Receiver<()>,
    finished: Arc<AtomicBool>,
}

impl ThreadAware for BlockingSetup {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.cx.relocate(source, destination);
    }
}

async fn start_blocking(setup: BlockingSetup) {
    drop(setup.cx.scheduler().spawn_blocking(move || {
        setup.started.send(()).unwrap();
        setup.release.recv().unwrap();
        setup.finished.store(true, Ordering::SeqCst);
    }));
}

#[test]
fn callback_runs_after_tasks_and_driver_teardown() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .build()
        .unwrap();
    let async_dropped = Arc::new(AtomicBool::new(false));
    let blocking_finished = Arc::new(AtomicBool::new(false));
    let queued_body_ran = Arc::new(AtomicBool::new(false));
    let driver_stopped = Arc::new(AtomicBool::new(false));
    let (release_blocking, blocking_gate) = mpsc::sync_channel(1);
    let (callback_entered, callback_observed) = mpsc::sync_channel(1);
    let (release_callback, callback_gate) = mpsc::sync_channel(1);

    let control = runtime
        .scheduler()
        .block_on({
            let async_dropped = Arc::clone(&async_dropped);
            let driver_stopped = Arc::clone(&driver_stopped);
            async move |cx| {
                let _ = cx.io().context::<ShutdownContext>(&DriverState(driver_stopped));
                let (async_started, async_ready) = oneshot();
                drop(cx.scheduler().spawn(async move |_| {
                    let _drop_flag = DropFlag(async_dropped);
                    async_started.send(()).unwrap();
                    pending::<()>().await;
                }));
                async_ready.await.unwrap();
                cx.control().clone()
            }
        })
        .unwrap();

    let (blocking_started, blocking_ready) = oneshot();
    let blocking_finished_in_task = Arc::clone(&blocking_finished);
    let running = runtime.scheduler().spawn_blocking(move || {
        blocking_started.send(()).unwrap();
        blocking_gate.recv().unwrap();
        blocking_finished_in_task.store(true, Ordering::SeqCst);
    });
    futures::executor::block_on(blocking_ready).unwrap();
    let queued_body = Arc::clone(&queued_body_ran);
    let queued = runtime.scheduler().spawn_blocking(move || {
        queued_body.store(true, Ordering::SeqCst);
    });

    control.on_shutdown({
        let async_dropped = Arc::clone(&async_dropped);
        let blocking_finished = Arc::clone(&blocking_finished);
        let queued_body_ran = Arc::clone(&queued_body_ran);
        let driver_stopped = Arc::clone(&driver_stopped);
        move || {
            callback_entered
                .send((
                    async_dropped.load(Ordering::SeqCst),
                    blocking_finished.load(Ordering::SeqCst),
                    queued_body_ran.load(Ordering::SeqCst),
                    driver_stopped.load(Ordering::SeqCst),
                ))
                .unwrap();
            callback_gate.recv().unwrap();
        }
    });

    let (stopped, shutdown_complete) = mpsc::sync_channel(1);
    let stopping = std::thread::spawn(move || {
        stopped.send(runtime.stop()).unwrap();
    });

    control.request_stop();
    assert_eq!(callback_observed.try_recv(), Err(mpsc::TryRecvError::Empty));
    release_blocking.send(()).unwrap();
    assert_eq!(callback_observed.recv_timeout(TEST_TIMEOUT).unwrap(), (true, true, false, true));
    assert!(matches!(shutdown_complete.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release_callback.send(()).unwrap();
    shutdown_complete.recv_timeout(TEST_TIMEOUT).unwrap().unwrap();
    stopping.join().unwrap();
    running.join().unwrap();
    assert!(queued.join().unwrap_err().is_shutdown());
}

#[test]
fn external_drop_waits_for_callback_completion() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let (callback_started, callback_ready) = mpsc::sync_channel(1);
    let (release_callback, callback_gate) = mpsc::sync_channel(1);
    control.on_shutdown(move || {
        callback_started.send(()).unwrap();
        callback_gate.recv().unwrap();
    });
    let (dropped, drop_complete) = mpsc::sync_channel(1);

    let dropping = std::thread::spawn(move || {
        drop(runtime);
        dropped.send(()).unwrap();
    });

    callback_ready.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(drop_complete.try_recv(), Err(mpsc::TryRecvError::Empty));
    release_callback.send(()).unwrap();
    drop_complete.recv_timeout(TEST_TIMEOUT).unwrap();
    dropping.join().unwrap();
}

#[test]
fn callback_submissions_are_rejected_without_running_their_bodies() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let async_body_ran = Arc::new(AtomicBool::new(false));
    let blocking_body_ran = Arc::new(AtomicBool::new(false));
    let (reported, result) = mpsc::sync_channel(1);

    runtime
        .scheduler()
        .block_on({
            let async_body_ran = Arc::clone(&async_body_ran);
            let blocking_body_ran = Arc::clone(&blocking_body_ran);
            async move |cx| {
                let scheduler = cx.scheduler().clone();
                cx.control().on_shutdown(move || {
                    let async_task = scheduler.spawn(async move |_| {
                        async_body_ran.store(true, Ordering::SeqCst);
                    });
                    let blocking_task = scheduler.spawn_blocking(move || {
                        blocking_body_ran.store(true, Ordering::SeqCst);
                    });
                    reported
                        .send((
                            futures::executor::block_on(async_task).unwrap_err().is_shutdown(),
                            blocking_task.join().unwrap_err().is_shutdown(),
                        ))
                        .unwrap();
                });
            }
        })
        .unwrap();

    runtime.stop().unwrap();

    assert_eq!(result.recv_timeout(TEST_TIMEOUT).unwrap(), (true, true));
    assert!(!async_body_ran.load(Ordering::SeqCst));
    assert!(!blocking_body_ran.load(Ordering::SeqCst));
}

#[test]
fn owner_drop_on_its_worker_defers_callback_until_worker_teardown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let (reported, result) = mpsc::sync_channel(1);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();

    scheduler
        .spawn(async move |cx| {
            cx.control().on_shutdown(move || reported.send(()).unwrap());
            drop(runtime);
        })
        .join()
        .unwrap();

    result.recv_timeout(TEST_TIMEOUT).unwrap();
}

#[test]
fn owner_stop_on_its_worker_defers_callback_until_worker_teardown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let (reported, result) = mpsc::sync_channel(1);
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();

    let outcome = scheduler
        .spawn(async move |cx| {
            cx.control().on_shutdown(move || reported.send(()).unwrap());
            runtime.stop()
        })
        .join()
        .unwrap();

    assert!(outcome.is_err());
    result.recv_timeout(TEST_TIMEOUT).unwrap();
}

#[test]
fn repeated_stop_requests_run_callback_once() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    control.on_shutdown(move || {
        observed_calls.fetch_add(1, Ordering::SeqCst);
    });

    control.request_stop();
    control.request_stop();
    runtime.stop().unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn foreign_worker_drop_returns_before_target_blocking_work_finishes() {
    execute_or_abandon(|| {
        let target = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let caller = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let blocking_finished = Arc::new(AtomicBool::new(false));
        let (blocking_started, blocking_ready) = oneshot();
        let (release_blocking, blocking_gate) = mpsc::sync_channel(1);
        let (callback_entered, callback_observed) = mpsc::sync_channel(1);
        let (target_dropped, drop_observed) = mpsc::sync_channel(1);

        let control = target.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
        let blocking_finished_in_task = Arc::clone(&blocking_finished);
        let blocking = target.scheduler().spawn_blocking(move || {
            blocking_started.send(()).unwrap();
            blocking_gate.recv().unwrap();
            blocking_finished_in_task.store(true, Ordering::SeqCst);
        });
        futures::executor::block_on(blocking_ready).unwrap();
        control.on_shutdown(move || {
            callback_entered.send(blocking_finished.load(Ordering::SeqCst)).unwrap();
        });

        let drop_target = caller.scheduler().spawn_anywhere(
            thread_aware::Unaware((target, target_dropped)),
            |_, thread_aware::Unaware((target, target_dropped))| async move {
                drop(target);
                target_dropped.send(()).unwrap();
            },
        );

        drop_observed.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(callback_observed.try_recv(), Err(mpsc::TryRecvError::Empty));
        release_blocking.send(()).unwrap();
        assert!(callback_observed.recv_timeout(TEST_TIMEOUT).unwrap());
        drop_target.join().unwrap();
        blocking.join().unwrap();
        caller.stop().unwrap();
    })
    .unwrap();
}

#[test]
fn owner_drop_in_its_blocking_callback_returns_before_callback_runs() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .build()
        .unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let (callback_entered, callback_observed) = mpsc::sync_channel(1);
    let (owner_dropped, drop_observed) = mpsc::sync_channel(1);
    let (release_blocking, blocking_gate) = mpsc::sync_channel(1);
    control.on_shutdown(move || callback_entered.send(()).unwrap());

    let blocking = scheduler.spawn_blocking(move || {
        drop(runtime);
        owner_dropped.send(()).unwrap();
        blocking_gate.recv().unwrap();
    });

    drop_observed.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(callback_observed.try_recv(), Err(mpsc::TryRecvError::Empty));
    release_blocking.send(()).unwrap();
    blocking.join().unwrap();
    callback_observed.recv_timeout(TEST_TIMEOUT).unwrap();
}

#[test]
fn foreign_blocking_callback_drop_returns_before_target_work_finishes() {
    execute_or_abandon(|| {
        let target = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .build()
            .unwrap();
        let caller = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .build()
            .unwrap();
        let target_control = target.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
        let (target_started, target_ready) = oneshot();
        let (release_target, target_gate) = mpsc::sync_channel(1);
        let target_blocking = target.scheduler().spawn_blocking(move || {
            target_started.send(()).unwrap();
            target_gate.recv().unwrap();
        });
        futures::executor::block_on(target_ready).unwrap();
        let (callback_entered, callback_observed) = mpsc::sync_channel(1);
        target_control.on_shutdown(move || callback_entered.send(()).unwrap());
        let (target_dropped, drop_observed) = mpsc::sync_channel(1);

        let drop_target = caller.scheduler().spawn_blocking(move || {
            drop(target);
            target_dropped.send(()).unwrap();
        });

        drop_observed.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(callback_observed.try_recv(), Err(mpsc::TryRecvError::Empty));
        release_target.send(()).unwrap();
        target_blocking.join().unwrap();
        callback_observed.recv_timeout(TEST_TIMEOUT).unwrap();
        drop_target.join().unwrap();
        caller.stop().unwrap();
    })
    .unwrap();
}

#[test]
fn thread_local_owner_drop_waits_for_every_worker_before_callback() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .blocking_pool(BlockingPoolPolicy::per_worker().max(1))
        .build()
        .unwrap();
    let (release_blocking, blocking_gate) = mpsc::sync_channel(1);
    let (blocking_started, blocking_ready) = oneshot();
    let blocking_finished = Arc::new(AtomicBool::new(false));
    let (owner_dropped, owner_drop_observed) = mpsc::sync_channel(1);
    let (reported, result) = mpsc::sync_channel(1);

    let (scheduler, control) = runtime
        .scheduler()
        .block_on(async |cx| {
            let workers = cx.control().workers().into_iter().cloned().collect::<Vec<_>>();
            let scheduler = cx
                .scheduler()
                .spawn_at(&workers[0], cx.clone(), |cx| async move { cx.scheduler().clone() })
                .await
                .unwrap();
            cx.scheduler()
                .spawn_at(
                    &workers[1],
                    BlockingSetup {
                        cx: cx.clone(),
                        started: blocking_started,
                        release: blocking_gate,
                        finished: Arc::clone(&blocking_finished),
                    },
                    start_blocking,
                )
                .await
                .unwrap();
            (scheduler, cx.control().clone())
        })
        .unwrap();
    futures::executor::block_on(blocking_ready).unwrap();
    control.on_shutdown(move || reported.send(blocking_finished.load(Ordering::SeqCst)).unwrap());

    scheduler
        .spawn(async move |_| {
            THREAD_LOCAL_RUNTIME.with(|owner| {
                assert!(
                    owner
                        .replace(Some(ThreadLocalRuntime {
                            runtime: Some(runtime),
                            dropped: owner_dropped,
                        }))
                        .is_none()
                );
            });
        })
        .join()
        .unwrap();

    control.request_stop();
    owner_drop_observed.recv_timeout(TEST_TIMEOUT).unwrap();
    release_blocking.send(()).unwrap();
    assert!(result.recv_timeout(TEST_TIMEOUT).unwrap());
}

#[test]
fn late_registration_on_a_foreign_worker_runs_on_a_non_worker_thread() {
    let target = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = target.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    target.stop().unwrap();
    let caller = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let (callback_started, callback_ready) = mpsc::sync_channel(1);
    let (release_callback, callback_gate) = mpsc::sync_channel(1);
    let (registration_returned, registration_observed) = mpsc::sync_channel(1);

    let registration = caller.scheduler().spawn_anywhere(
        thread_aware::Unaware((control, callback_started, callback_gate, registration_returned)),
        |cx, thread_aware::Unaware((control, callback_started, callback_gate, registration_returned))| async move {
            let worker = cx.thread().id();
            control.on_shutdown(move || {
                callback_started.send((worker, std::thread::current().id())).unwrap();
                callback_gate.recv().unwrap();
            });
            registration_returned.send(()).unwrap();
        },
    );

    let (worker, callback_thread) = callback_ready.recv_timeout(TEST_TIMEOUT).unwrap();
    registration_observed.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_ne!(worker, callback_thread);
    release_callback.send(()).unwrap();
    registration.join().unwrap();
    caller.stop().unwrap();
}

#[test]
fn callback_panic_does_not_skip_remaining_callbacks() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let (reported, result) = mpsc::sync_channel(1);
    control.on_shutdown(|| panic!("shutdown callback panic"));
    control.on_shutdown(move || reported.send(()).unwrap());

    let panic = catch_unwind(AssertUnwindSafe(|| runtime.stop())).unwrap_err();

    result.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"shutdown callback panic"));
}
