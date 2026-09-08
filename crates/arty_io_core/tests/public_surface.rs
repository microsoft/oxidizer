// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;
use std::{fmt, thread};

use arty_io_core::{Driver, DriverContext, DriverInit, DriverProvider, SystemTasks};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

assert_impl_all!(DriverInit: Send, Sync, fmt::Debug);
assert_impl_all!(SystemTasks: Clone, Send, Sync, fmt::Debug);

#[test]
fn public_handles_have_expected_traits() {
    let system_tasks = SystemTasks::new(|task| task());

    let _: &SystemTasks = &system_tasks;
    assert!(format!("{system_tasks:?}").contains("SystemTasks"));
}

#[test]
fn driver_can_remain_thread_local() {
    assert_not_impl_any!(LocalDriver: Send, Sync);

    fn assert_driver<T: Driver>() {}
    assert_driver::<LocalDriver>();
}

#[test]
fn driver_is_boxable_with_its_context_type() {
    let driver: Box<dyn Driver<Context = TestContext>> = Box::new(LocalDriver::new(Rc::default()));

    assert_eq!(driver.context(), TestContext(7));
}

#[test]
fn driver_init_exposes_runtime_facilities() {
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_by_callback = Arc::clone(&accepted);
    let system_tasks = SystemTasks::new(move |task| {
        accepted_by_callback.fetch_add(1, Ordering::Relaxed);
        task();
    });
    let worker = worker_thread();
    let init = DriverInit::new(worker.clone(), system_tasks);

    init.system_tasks().spawn(|| {});

    assert_eq!(init.thread(), &worker);
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    assert!(format!("{init:?}").contains("DriverInit"));
}

#[test]
fn provider_creation_is_typed_and_infallible() {
    let init = driver_init();
    let driver = TestProvider.create(init);
    assert_eq!(driver.context(), TestContext(7));
}

#[test]
fn context_type_selects_provider_and_driver() {
    fn provider_for<C: DriverContext>() -> C::Provider {
        C::provider()
    }

    let provider: TestProvider = provider_for::<TestContext>();
    let driver = provider.create(driver_init());

    assert_eq!(driver.context(), TestContext(7));
}

#[test]
fn shutdown_is_idempotent_and_pollable() {
    let state = Rc::new(ShutdownState::default());
    let mut driver = LocalDriver::new(Rc::clone(&state));
    let wake_count = Arc::new(CountingWake::default());
    let waker = Waker::from(Arc::clone(&wake_count));
    let mut cx = Context::from_waker(&waker);

    driver.begin_shutdown();
    driver.begin_shutdown();
    assert_eq!(state.begin_calls.get(), 1);
    assert_eq!(driver.poll_shutdown(&mut cx), Poll::Pending);
    assert_eq!(state.poll_calls.get(), 1);
    assert_eq!(wake_count.count.load(Ordering::Relaxed), 1);

    assert_eq!(driver.poll_shutdown(&mut cx), Poll::Ready(()));
    assert_eq!(state.begin_calls.get(), 1);
    assert_eq!(state.poll_calls.get(), 2);
    assert_eq!(driver.poll_shutdown(&mut cx), Poll::Ready(()));
}

#[test]
fn shutdown_waits_for_active_operations_not_contexts() {
    let mut driver = LeaseDriver::new();
    let context = driver.context();
    let operation = context.begin_operation().expect("admission is open before shutdown");
    let wake_count = Arc::new(CountingWake::default());
    let waker = Waker::from(Arc::clone(&wake_count));
    let mut cx = Context::from_waker(&waker);

    driver.begin_shutdown();

    assert!(context.begin_operation().is_none());
    assert!(driver.context().begin_operation().is_none());
    assert_eq!(driver.poll_shutdown(&mut cx), Poll::Pending);

    drop(operation);
    assert_eq!(wake_count.count.load(Ordering::Relaxed), 1);
    assert_eq!(driver.poll_shutdown(&mut cx), Poll::Ready(()));

    drop(driver);
    assert!(context.begin_operation().is_none());
}

#[test]
fn dropping_driver_closes_context_admission() {
    let driver = LeaseDriver::new();
    let context = driver.context();

    drop(driver);

    assert!(context.begin_operation().is_none());
}

#[test]
fn polling_shutdown_closes_admission() {
    let mut driver = LeaseDriver::new();
    let context = driver.context();
    let mut cx = Context::from_waker(Waker::noop());

    assert_eq!(driver.poll_shutdown(&mut cx), Poll::Ready(()));
    assert!(context.begin_operation().is_none());
}

#[test]
fn different_driver_types_have_distinct_identity() {
    use std::any::TypeId;

    assert_ne!(TypeId::of::<version_one::Driver>(), TypeId::of::<version_two::Driver>());
}

#[test]
fn completion_processing_supports_latched_wakeup() {
    let mut driver = LocalDriver::new(Rc::default());

    driver.waker().wake_by_ref();
    driver.process_completions(Duration::MAX);

    assert_eq!(driver.completion_queue.waits.load(Ordering::Relaxed), 1);
}

#[test]
fn waker_remains_valid_after_driver_drop() {
    let driver = LocalDriver::new(Rc::default());
    let waker = driver.waker();

    drop(driver);
    waker.wake();
}

#[derive(Debug, Default)]
struct Latch {
    raised: Mutex<bool>,
    changed: Condvar,
}

impl Wake for Latch {
    fn wake(self: Arc<Self>) {
        let mut raised = self.raised.lock().unwrap_or_else(PoisonError::into_inner);
        *raised = true;
        self.changed.notify_one();
    }
}

#[derive(Debug, Default)]
struct TestCompletionQueue {
    latch: Arc<Latch>,
    waits: AtomicUsize,
}

impl TestCompletionQueue {
    fn process_completions(&self, max_wait: Duration) {
        self.waits.fetch_add(1, Ordering::Relaxed);
        let mut raised = self.latch.raised.lock().unwrap_or_else(PoisonError::into_inner);

        if *raised {
            *raised = false;
            return;
        }

        if max_wait.is_zero() {
            return;
        }

        if max_wait == Duration::MAX {
            while !*raised {
                raised = self.latch.changed.wait(raised).unwrap_or_else(PoisonError::into_inner);
            }
        } else {
            let (next, _) = self
                .latch
                .changed
                .wait_timeout(raised, max_wait)
                .unwrap_or_else(PoisonError::into_inner);
            raised = next;
        }

        *raised = false;
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.latch))
    }
}

#[derive(Debug, Default)]
struct ShutdownState {
    started: Cell<bool>,
    begin_calls: Cell<usize>,
    poll_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    context: TestContext,
    completion_queue: TestCompletionQueue,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self {
            state,
            context: TestContext(7),
            completion_queue: TestCompletionQueue::default(),
        }
    }
}

impl Driver for LocalDriver {
    type Context = TestContext;

    fn context(&self) -> Self::Context {
        self.context.clone()
    }

    fn process_completions(&mut self, max_wait: Duration) {
        self.completion_queue.process_completions(max_wait);
    }

    fn waker(&self) -> Waker {
        self.completion_queue.waker()
    }

    fn begin_shutdown(&mut self) {
        if self.state.started.replace(true) {
            return;
        }

        self.state.begin_calls.update(|calls| calls + 1);
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.begin_shutdown();
        self.state.poll_calls.update(|calls| calls + 1);

        if self.state.poll_calls.get() == 1 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestContext(u8);

impl ThreadAware for TestContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverContext for TestContext {
    type Provider = TestProvider;

    fn provider() -> Self::Provider {
        TestProvider
    }
}

#[derive(Clone, Debug)]
struct TestProvider;

impl ThreadAware for TestProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for TestProvider {
    type Context = TestContext;
    type Driver = LocalDriver;

    fn create(self, _init: DriverInit) -> Self::Driver {
        LocalDriver::new(Rc::default())
    }
}

#[derive(Clone, Debug)]
struct LeaseContext {
    state: Arc<LeaseState>,
}

impl LeaseContext {
    fn begin_operation(&self) -> Option<OperationLease> {
        let mut lifecycle = self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);

        if lifecycle.shutdown_started {
            return None;
        }

        lifecycle.active_operations += 1;
        Some(OperationLease {
            state: Arc::clone(&self.state),
        })
    }
}

impl ThreadAware for LeaseContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverContext for LeaseContext {
    type Provider = LeaseProvider;

    fn provider() -> Self::Provider {
        LeaseProvider
    }
}

#[derive(Clone, Debug)]
struct LeaseProvider;

impl ThreadAware for LeaseProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for LeaseProvider {
    type Context = LeaseContext;
    type Driver = LeaseDriver;

    fn create(self, _init: DriverInit) -> Self::Driver {
        LeaseDriver::new()
    }
}

#[derive(Debug, Default)]
struct LeaseState {
    lifecycle: Mutex<LeaseLifecycle>,
}

#[derive(Debug, Default)]
struct LeaseLifecycle {
    shutdown_started: bool,
    active_operations: usize,
    shutdown_waker: Option<Waker>,
}

#[derive(Debug)]
struct OperationLease {
    state: Arc<LeaseState>,
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let mut lifecycle = self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);

        lifecycle.active_operations -= 1;
        let waker = (lifecycle.active_operations == 0)
            .then(|| lifecycle.shutdown_waker.take())
            .flatten();
        drop(lifecycle);

        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[derive(Debug)]
struct LeaseDriver {
    state: Arc<LeaseState>,
}

impl LeaseDriver {
    fn new() -> Self {
        Self { state: Arc::default() }
    }
}

impl Drop for LeaseDriver {
    fn drop(&mut self) {
        self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner).shutdown_started = true;
    }
}

impl Driver for LeaseDriver {
    type Context = LeaseContext;

    fn context(&self) -> Self::Context {
        LeaseContext {
            state: Arc::clone(&self.state),
        }
    }

    fn process_completions(&mut self, _max_wait: Duration) {}

    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn begin_shutdown(&mut self) {
        self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner).shutdown_started = true;
    }

    fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.begin_shutdown();
        let mut lifecycle = self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);

        if lifecycle.active_operations == 0 {
            Poll::Ready(())
        } else {
            lifecycle.shutdown_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[derive(Debug, Default)]
struct CountingWake {
    count: AtomicUsize,
}

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }
}

fn driver_init() -> DriverInit {
    DriverInit::new(worker_thread(), SystemTasks::new(|task| task()))
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
