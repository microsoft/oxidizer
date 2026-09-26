// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

use std::cell::Cell;
use std::error::Error;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};
use std::{fmt, io, thread};

use arty_io_core::{
    Coordinator, Cycle, Driver, DriverError, DriverHandle, DriverOptions, DriverProvider, DriverRole, IoContext, ProviderOptions,
    ShutdownError, SystemTaskSpawner,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

assert_impl_all!(DriverOptions<'static>: fmt::Debug);
assert_not_impl_any!(DriverOptions<'static>: Send, Sync);
assert_not_impl_any!(Cycle<'static>: Send, Sync);
assert_impl_all!(DriverHandle<'static>: Copy, fmt::Debug);
assert_not_impl_any!(DriverHandle<'static>: Send, Sync);
assert_impl_all!(DriverRole: Copy, Send, Sync, fmt::Debug, Eq);
assert_impl_all!(ProviderOptions: Send, Sync, fmt::Debug);
assert_impl_all!(DriverError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(ShutdownError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(SystemTaskSpawner: Clone, Send, Sync, fmt::Debug);

#[test]
fn system_task_spawner_debug() {
    let spawner = SystemTaskSpawner::from_fn(|task| task());

    assert_eq!(format!("{spawner:?}"), "SystemTaskSpawner { .. }");
}

#[test]
fn public_options_expose_runtime_facilities() {
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_by_callback = Arc::clone(&accepted);
    let spawner = SystemTaskSpawner::from_fn(move |task| {
        accepted_by_callback.fetch_add(1, Ordering::Relaxed);
        task();
    });
    let worker = worker_thread();
    let options = DriverOptions::new(worker.clone(), spawner, Vec::new(), DriverRole::Primary);

    options.spawner().spawn(|| {});

    assert_eq!(options.thread(), &worker);
    assert_eq!(options.role(), DriverRole::Primary);
    assert!(options.drivers().is_empty());
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    assert!(format!("{options:?}").contains("DriverOptions"));
    assert!(format!("{:?}", ProviderOptions::new()).contains("ProviderOptions"));
}

#[test]
fn driver_options_expose_drivers_registered_on_the_thread() {
    let existing_driver = LocalDriver::new(Rc::default());
    let handles = vec![existing_driver.handle()];
    let options = DriverOptions::new(
        worker_thread(),
        SystemTaskSpawner::from_fn(|task| task()),
        handles,
        DriverRole::Secondary,
    );

    let drivers = options.drivers();

    assert_eq!(drivers.len(), 1);
    assert!(drivers[0].handle().is::<LocalDriver>());
    assert!(format!("{options:?}").contains("driver_count"));
    assert!(format!("{:?}", drivers[0]).contains("DriverHandle"));
}

#[test]
fn driver_is_notified_when_another_driver_is_registered() {
    let mut driver = LocalDriver::new(Rc::default());
    let registered_driver = LeaseDriver::new(Arc::default());

    driver.on_peer_registered(registered_driver.handle());

    assert_eq!(driver.registered_driver_count, 1);
}

#[test]
fn driver_can_remain_thread_local() {
    assert_not_impl_any!(LocalDriver: Send, Sync);

    fn assert_driver<T: Driver>() {}
    assert_driver::<LocalDriver>();
}

#[test]
fn driver_is_boxable() {
    let driver: Box<dyn Driver> = Box::new(LocalDriver::new(Rc::default()));

    assert!(driver.handle().handle().is::<LocalDriver>());
}

#[test]
fn provider_creation_uses_both_options() {
    fn provider_for<C: IoContext>(options: ProviderOptions) -> C::Provider {
        C::provider(options)
    }

    let provider: TestProvider = provider_for::<TestContext>(ProviderOptions::new());
    let (driver, context) = provider.create(driver_options()).unwrap();

    assert!(driver.handle().handle().is::<LocalDriver>());
    assert_eq!(context, TestContext(7));
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
    let (driver, context) = LeaseProvider.create(driver_options()).unwrap();
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
    let (driver, context) = LeaseProvider.create(driver_options()).unwrap();

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
fn shutdown_error_can_be_created_from_source() {
    let error = ShutdownError::from_source(io::Error::other("completion queue failed"));

    assert!(error.to_string().contains("shutdown"));
    assert!(error.source().is_some_and(|cause| cause.to_string().contains("completion queue")));
}

#[test]
fn driver_error_can_be_created_from_message_and_source() {
    let error = DriverError::from_message("native driver failed");
    assert_eq!(error.to_string(), "native driver failed");
    assert!(error.source().is_none());

    let error = DriverError::from_source(io::Error::other("completion queue failed"));
    assert_eq!(error.to_string(), "i/o driver failed");
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
    let coordinator = Coordinator::new();

    coordinator.interrupt_waker().wake_by_ref();
    driver
        .execute_cycle(Cycle::new(Instant::now(), Duration::MAX, &coordinator))
        .unwrap();

    assert_eq!(driver.completion_queue.waits.load(Ordering::Relaxed), 1);
}

#[test]
fn completion_cycle_uses_one_time_snapshot() {
    let mut first_driver = LocalDriver::new(Rc::default());
    let mut second_driver = LocalDriver::new(Rc::default());
    let cycle_start = Instant::now();
    let coordinator = Coordinator::new();

    first_driver
        .execute_cycle(Cycle::new(cycle_start, Duration::ZERO, &coordinator))
        .unwrap();
    second_driver
        .execute_cycle(Cycle::new(cycle_start, Duration::ZERO, &coordinator))
        .unwrap();

    assert_eq!(*first_driver.completion_queue.cycle_start.lock().unwrap(), Some(cycle_start));
    assert_eq!(*second_driver.completion_queue.cycle_start.lock().unwrap(), Some(cycle_start));
}

#[test]
fn non_blocking_completion_processing_preserves_latched_interrupt() {
    let mut driver = LocalDriver::new(Rc::default());
    let coordinator = Coordinator::new();

    coordinator.interrupt_waker().wake_by_ref();
    driver
        .execute_cycle(Cycle::new(Instant::now(), Duration::ZERO, &coordinator))
        .unwrap();

    assert!(*driver.completion_queue.latch.raised.lock().unwrap());

    driver
        .execute_cycle(Cycle::new(Instant::now(), Duration::MAX, &coordinator))
        .unwrap();

    assert!(!*driver.completion_queue.latch.raised.lock().unwrap());
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
    cycle_start: Mutex<Option<Instant>>,
}

impl TestCompletionQueue {
    fn process_completions(&self, cycle: &Cycle) {
        self.waits.fetch_add(1, Ordering::Relaxed);
        *self.cycle_start.lock().unwrap_or_else(PoisonError::into_inner) = Some(cycle.started_at());

        if cycle.max_wait().is_zero() {
            return;
        }

        let mut raised = self.latch.raised.lock().unwrap_or_else(PoisonError::into_inner);

        if *raised {
            *raised = false;
            return;
        }

        if cycle.max_wait() == Duration::MAX {
            while !*raised {
                raised = self.latch.changed.wait(raised).unwrap_or_else(PoisonError::into_inner);
            }
        } else {
            let (next, _) = self
                .latch
                .changed
                .wait_timeout(raised, cycle.max_wait())
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
    shutdown_calls: Cell<usize>,
    drop_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    completion_queue: TestCompletionQueue,
    owned_resource: Option<Box<()>>,
    registered_driver_count: usize,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self {
            state,
            completion_queue: TestCompletionQueue::default(),
            owned_resource: Some(Box::new(())),
            registered_driver_count: 0,
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
    fn handle(&self) -> DriverHandle<'_> {
        DriverHandle::new(self)
    }

    fn on_peer_registered(&mut self, peer: DriverHandle<'_>) {
        if peer.handle().is::<LeaseDriver>() {
            self.registered_driver_count += 1;
        }
    }

    fn execute_cycle(&mut self, cycle: Cycle<'_>) -> Result<(), DriverError> {
        let mut token = cycle.start_work();
        token.on_interrupt(self.completion_queue.waker());
        self.completion_queue.process_completions(&cycle);
        Ok(())
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

    fn provider(_options: ProviderOptions) -> Self::Provider {
        TestProvider
    }
}

#[derive(Clone, Debug)]
struct TestProvider;

impl ThreadAware for TestProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for TestProvider {
    const CAN_BE_PRIMARY: bool = true;

    type Context = TestContext;
    type Driver = LocalDriver;

    fn create(self, _options: DriverOptions<'_>) -> Result<(Self::Driver, Self::Context), DriverError> {
        Ok((LocalDriver::new(Rc::default()), TestContext(7)))
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

    fn provider(_options: ProviderOptions) -> Self::Provider {
        LeaseProvider
    }
}

#[derive(Clone, Debug)]
struct LeaseProvider;

impl ThreadAware for LeaseProvider {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl DriverProvider for LeaseProvider {
    const CAN_BE_PRIMARY: bool = false;

    type Context = LeaseContext;
    type Driver = LeaseDriver;

    fn create(self, _options: DriverOptions<'_>) -> Result<(Self::Driver, Self::Context), DriverError> {
        let state = Arc::default();
        Ok((LeaseDriver::new(Arc::clone(&state)), LeaseContext { state }))
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
    fn new(state: Arc<LeaseState>) -> Self {
        Self { state }
    }
}

impl Drop for LeaseDriver {
    fn drop(&mut self) {
        self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner).shutdown_started = true;
        self.state.changed.notify_all();
    }
}

impl Driver for LeaseDriver {
    fn handle(&self) -> DriverHandle<'_> {
        DriverHandle::new(self)
    }

    fn on_peer_registered(&mut self, _peer: DriverHandle<'_>) {}

    fn execute_cycle(&mut self, _cycle: Cycle<'_>) -> Result<(), DriverError> {
        Ok(())
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

fn driver_options() -> DriverOptions<'static> {
    DriverOptions::new(
        worker_thread(),
        SystemTaskSpawner::from_fn(|task| task()),
        Vec::new(),
        DriverRole::Secondary,
    )
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
