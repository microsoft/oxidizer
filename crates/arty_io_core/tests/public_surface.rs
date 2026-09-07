// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

use std::cell::Cell;
use std::fmt;
use std::pin::pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::{
    BlockingTask, BlockingTaskSpawner, Driver, DriverInit, DriverProvider, Parker, Thread,
    ThreadAware, WaitingPoint,
};
use static_assertions::{assert_impl_all, assert_not_impl_any, assert_obj_safe};

assert_obj_safe!(Parker);
assert_obj_safe!(BlockingTaskSpawner);
assert_impl_all!(DriverInit: Clone, Send, Sync, fmt::Debug);

#[test]
fn public_traits_have_expected_object_safety() {
    let mut parker = TestParker::default();
    let blocking_tasks = InlineBlockingTasks::default();

    let _: &mut dyn Parker = &mut parker;
    let _: &dyn BlockingTaskSpawner = &blocking_tasks;
}

#[test]
fn driver_can_remain_thread_local() {
    assert_not_impl_any!(LocalDriver: Send, Sync);

    fn assert_driver<T: Driver>() {}
    assert_driver::<LocalDriver>();
}

#[test]
fn driver_init_exposes_runtime_facilities() {
    let blocking_tasks = Arc::new(InlineBlockingTasks::default());
    let worker = worker_thread();
    let init = DriverInit::new(
        worker.clone(),
        blocking_tasks.clone(),
        WaitingPoint::Available,
    );

    init.blocking_tasks().spawn(Box::new(|| {}));

    assert_eq!(init.thread(), &worker);
    assert_eq!(init.waiting_point(), WaitingPoint::Available);
    assert_eq!(blocking_tasks.accepted.load(Ordering::Relaxed), 1);
}

#[test]
fn provider_creation_is_typed_and_fallible() {
    let init = driver_init(WaitingPoint::Unavailable);
    let driver = TestProvider { fail: false }.create(init).unwrap();
    assert_eq!(driver.context(), 7);

    let error = TestProvider { fail: true }
        .create(driver_init(WaitingPoint::Unavailable))
        .unwrap_err();
    assert_eq!(error.to_string(), "driver creation failed");
}

#[test]
fn shutdown_future_begins_and_polls_shutdown() {
    let state = Rc::new(ShutdownState::default());
    let mut driver = LocalDriver::new(Rc::clone(&state));
    let wake_count = Arc::new(CountingWake::default());
    let waker = Waker::from(wake_count.clone());
    let mut cx = Context::from_waker(&waker);
    let mut shutdown = pin!(driver.shutdown());

    assert_eq!(shutdown.as_mut().poll(&mut cx), Poll::Pending);
    assert_eq!(state.begin_calls.get(), 1);
    assert_eq!(state.poll_calls.get(), 1);
    assert_eq!(wake_count.count.load(Ordering::Relaxed), 1);

    assert_eq!(shutdown.as_mut().poll(&mut cx), Poll::Ready(()));
    assert_eq!(state.begin_calls.get(), 1);
    assert_eq!(state.poll_calls.get(), 2);
}

#[test]
fn different_driver_types_have_distinct_identity() {
    use std::any::TypeId;

    assert_ne!(
        TypeId::of::<version_one::Driver>(),
        TypeId::of::<version_two::Driver>()
    );
}

#[test]
fn parker_contract_supports_latched_wakeup() {
    let mut parker = TestParker::default();

    parker.waker().wake_by_ref();
    parker.park(Duration::MAX);

    assert_eq!(parker.waits, 1);
}

#[derive(Default)]
struct InlineBlockingTasks {
    accepted: AtomicUsize,
}

impl BlockingTaskSpawner for InlineBlockingTasks {
    fn spawn(&self, task: BlockingTask) {
        self.accepted.fetch_add(1, Ordering::Relaxed);
        task();
    }
}

#[derive(Debug, Default)]
struct Latch {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl Wake for Latch {
    fn wake(self: Arc<Self>) {
        let mut raised = self.raised.lock().unwrap();
        *raised = true;
        self.changed.notify_one();
    }
}

#[derive(Debug, Default)]
struct TestParker {
    latch: Arc<Latch>,
    waits: usize,
}

impl Parker for TestParker {
    fn park(&mut self, max_wait: Duration) {
        self.waits += 1;
        let mut raised = self.latch.raised.lock().unwrap();

        if *raised {
            *raised = false;
            return;
        }

        if max_wait.is_zero() {
            return;
        }

        if max_wait == Duration::MAX {
            while !*raised {
                raised = self.latch.changed.wait(raised).unwrap();
            }
        } else {
            let (next, _) = self.latch.changed.wait_timeout(raised, max_wait).unwrap();
            raised = next;
        }

        *raised = false;
    }

    fn waker(&self) -> Waker {
        Waker::from(self.latch.clone())
    }
}

#[derive(Debug, Default)]
struct ShutdownState {
    begin_calls: Cell<usize>,
    poll_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    context: u8,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self { state, context: 7 }
    }
}

impl Driver for LocalDriver {
    type Context = u8;

    fn context(&self) -> Self::Context {
        self.context
    }

    fn begin_shutdown(&mut self) {
        self.state.begin_calls.update(|calls| calls + 1);
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.state.poll_calls.update(|calls| calls + 1);

        if self.state.poll_calls.get() == 1 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }
}

#[derive(Clone, Debug)]
struct TestProvider {
    fail: bool,
}

impl ThreadAware for TestProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for TestProvider {
    type Driver = LocalDriver;
    type Error = CreateError;

    fn create(self, _init: DriverInit) -> Result<Self::Driver, Self::Error> {
        if self.fail {
            Err(CreateError)
        } else {
            Ok(LocalDriver::new(Rc::default()))
        }
    }
}

#[derive(Debug)]
struct CreateError;

impl fmt::Display for CreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("driver creation failed")
    }
}

impl std::error::Error for CreateError {}

#[derive(Debug, Default)]
struct CountingWake {
    count: AtomicUsize,
}

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

fn driver_init(waiting_point: WaitingPoint) -> DriverInit {
    DriverInit::new(worker_thread(), Arc::new(InlineBlockingTasks::default()), waiting_point)
}

fn worker_thread() -> Thread {
    let owner = thread_aware_core::__private::v1::new_owner();
    let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
    thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node)
}

mod version_one {
    pub(super) struct Driver;
}

mod version_two {
    pub(super) struct Driver;
}
