// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

use std::cell::Cell;
use std::error::Error;
use std::io;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};
use std::{fmt, thread};

use arty_io_core::{Driver, DriverContext, DriverProvider, IoContext, ProviderContext, ShutdownError, SystemTasks};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

assert_impl_all!(DriverContext: Send, Sync, fmt::Debug);
assert_impl_all!(ProviderContext: Send, Sync, fmt::Debug);
assert_impl_all!(ShutdownError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(SystemTasks: Clone, Send, Sync, fmt::Debug);

#[test]
fn public_contexts_expose_runtime_facilities() {
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_by_callback = Arc::clone(&accepted);
    let system_tasks = SystemTasks::new(move |task| {
        accepted_by_callback.fetch_add(1, Ordering::Relaxed);
        task();
    });
    let worker = worker_thread();
    let context = DriverContext::new(worker.clone(), system_tasks);

    context.system_tasks().spawn(|| {});

    assert_eq!(context.thread(), &worker);
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    assert!(format!("{context:?}").contains("DriverContext"));
    assert!(format!("{:?}", ProviderContext::default()).contains("ProviderContext"));
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
fn provider_creation_uses_both_contexts() {
    fn provider_for<C: IoContext>(context: ProviderContext) -> C::Provider {
        C::provider(context)
    }

    let provider: TestProvider = provider_for::<TestContext>(ProviderContext::new());
    let driver = provider.create(driver_context());

    assert_eq!(driver.context(), TestContext(7));
}

#[test]
fn shutdown_consumes_the_driver() {
    let state = Rc::new(ShutdownState::default());
    let driver = LocalDriver::new(Rc::clone(&state));

    driver.shutdown().unwrap();

    assert_eq!(state.shutdown_calls.get(), 1);
    assert_eq!(state.drop_calls.get(), 1);
}

#[test]
fn shutdown_waits_for_active_operations_not_contexts() {
    let driver = LeaseDriver::new();
    let context = driver.context();
    let operation = context.begin_operation().expect("admission is open before shutdown");
    let (shutdown_tx, shutdown_rx) = mpsc::channel();

    let shutdown_thread = thread::spawn(move || {
        shutdown_tx.send(driver.shutdown()).unwrap();
    });

    {
        let mut lifecycle = context.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);
        while !lifecycle.shutdown_started {
            lifecycle = context.state.changed.wait(lifecycle).unwrap_or_else(PoisonError::into_inner);
        }
    }

    assert!(context.begin_operation().is_none());
    assert!(matches!(shutdown_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

    drop(operation);

    shutdown_rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    shutdown_thread.join().unwrap();
}

#[test]
fn dropping_driver_closes_context_admission() {
    let driver = LeaseDriver::new();
    let context = driver.context();

    drop(driver);

    assert!(context.begin_operation().is_none());
}

#[test]
fn shutdown_error_can_be_created_from_message() {
    let error = ShutdownError::from_message("driver drain timed out");

    assert!(error.to_string().contains("timed out"));
    assert!(error.source().is_none());
}

#[test]
fn shutdown_error_can_be_created_from_cause() {
    let error = ShutdownError::from_cause(io::Error::other("completion queue failed"));

    assert!(error.to_string().contains("shutdown"));
    assert!(error.source().is_some_and(|cause| cause.to_string().contains("completion queue")));
}

#[test]
fn different_driver_types_have_distinct_identity() {
    use std::any::TypeId;

    assert_ne!(TypeId::of::<version_one::Driver>(), TypeId::of::<version_two::Driver>());
}

#[test]
fn completion_processing_supports_latched_interrupt() {
    let mut driver = LocalDriver::new(Rc::default());

    driver.interruptor().wake_by_ref();
    driver.process_completions(Duration::MAX);

    assert_eq!(driver.completion_queue.waits.load(Ordering::Relaxed), 1);
}

#[test]
fn non_blocking_completion_processing_preserves_latched_interrupt() {
    let mut driver = LocalDriver::new(Rc::default());

    driver.interruptor().wake_by_ref();
    driver.process_completions(Duration::ZERO);

    assert!(*driver.completion_queue.latch.raised.lock().unwrap());

    driver.process_completions(Duration::MAX);

    assert!(!*driver.completion_queue.latch.raised.lock().unwrap());
}

#[test]
fn interruptor_remains_valid_after_driver_drop() {
    let driver = LocalDriver::new(Rc::default());
    let interruptor = driver.interruptor();

    drop(driver);
    interruptor.wake();
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

        if max_wait.is_zero() {
            return;
        }

        let mut raised = self.latch.raised.lock().unwrap_or_else(PoisonError::into_inner);

        if *raised {
            *raised = false;
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

    fn interruptor(&self) -> Waker {
        Waker::from(Arc::clone(&self.latch))
    }
}

#[derive(Debug, Default)]
struct ShutdownState {
    shutdown_calls: Cell<usize>,
    drop_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    context: TestContext,
    completion_queue: TestCompletionQueue,
    owned_resource: Option<Box<()>>,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self {
            state,
            context: TestContext(7),
            completion_queue: TestCompletionQueue::default(),
            owned_resource: Some(Box::new(())),
        }
    }
}

impl Drop for LocalDriver {
    fn drop(&mut self) {
        let _ = self.owned_resource.take();
        self.state.drop_calls.update(|calls| calls + 1);
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

    fn interruptor(&self) -> Waker {
        self.completion_queue.interruptor()
    }

    fn shutdown(mut self) -> Result<(), ShutdownError> {
        let _ = self.owned_resource.take();
        self.state.shutdown_calls.update(|calls| calls + 1);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestContext(u8);

impl ThreadAware for TestContext {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl IoContext for TestContext {
    type Provider = TestProvider;

    fn provider(_context: ProviderContext) -> Self::Provider {
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

    fn create(self, _context: DriverContext) -> Self::Driver {
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

impl IoContext for LeaseContext {
    type Provider = LeaseProvider;

    fn provider(_context: ProviderContext) -> Self::Provider {
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

    fn create(self, _context: DriverContext) -> Self::Driver {
        LeaseDriver::new()
    }
}

#[derive(Debug, Default)]
struct LeaseState {
    lifecycle: Mutex<LeaseLifecycle>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct LeaseLifecycle {
    shutdown_started: bool,
    active_operations: usize,
}

#[derive(Debug)]
struct OperationLease {
    state: Arc<LeaseState>,
}

impl Drop for OperationLease {
    fn drop(&mut self) {
        let mut lifecycle = self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);

        lifecycle.active_operations -= 1;
        let drained = lifecycle.active_operations == 0;
        drop(lifecycle);

        if drained {
            self.state.changed.notify_all();
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
        self.state.changed.notify_all();
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

    fn interruptor(&self) -> Waker {
        Waker::noop().clone()
    }

    fn shutdown(self) -> Result<(), ShutdownError> {
        const TIMEOUT: Duration = Duration::from_secs(1);

        let mut lifecycle = self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);
        lifecycle.shutdown_started = true;
        self.state.changed.notify_all();

        let deadline = Instant::now() + TIMEOUT;
        while lifecycle.active_operations != 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ShutdownError::from_message(
                    "active operations did not drain before the shutdown deadline",
                ));
            }

            let (next, wait_result) = self
                .state
                .changed
                .wait_timeout(lifecycle, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            lifecycle = next;

            if wait_result.timed_out() && lifecycle.active_operations != 0 {
                return Err(ShutdownError::from_message(
                    "active operations did not drain before the shutdown deadline",
                ));
            }
        }

        Ok(())
    }
}

fn driver_context() -> DriverContext {
    DriverContext::new(worker_thread(), SystemTasks::new(|task| task()))
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
