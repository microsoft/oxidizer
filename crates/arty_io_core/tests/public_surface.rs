// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::cell::Cell;
use std::error::Error;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};
use std::{fmt, io, thread};

use arty_io_core::{
    Cycle, Driver, DriverError, DriverInstance, DriverOptions, DriverProvider, DriverRole, IoContext, PrimaryDriver, ProviderOptions,
    SecondaryDriver, ShutdownError, SystemTaskSpawner,
};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

assert_impl_all!(DriverOptions: Send, Sync, fmt::Debug);
assert_not_impl_any!(Cycle: Copy, Send, Sync);
assert_impl_all!(Cycle: fmt::Debug);
assert_impl_all!(DriverRole: Copy, Send, Sync, fmt::Debug, Eq);
assert_impl_all!(ProviderOptions: Send, Sync, fmt::Debug);
assert_impl_all!(DriverError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(ShutdownError: Send, Sync, fmt::Debug, fmt::Display, Error);
assert_impl_all!(SystemTaskSpawner: Clone, Send, Sync, fmt::Debug);

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

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
    let options = DriverOptions::new(worker.clone(), spawner, vec![DriverRole::Primary, DriverRole::Secondary]);

    options.spawner().spawn(|| {});

    assert_eq!(options.thread(), &worker);
    assert_eq!(options.allowed_roles(), &[DriverRole::Primary, DriverRole::Secondary]);
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    assert!(format!("{options:?}").contains("DriverOptions"));
    assert!(format!("{:?}", ProviderOptions::new()).contains("ProviderOptions"));
}

#[test]
fn driver_can_remain_thread_local() {
    assert_not_impl_any!(LocalDriver: Send, Sync);

    fn assert_driver<T: Driver>() {}
    assert_driver::<LocalDriver>();
}

#[test]
fn driver_is_boxable() {
    let state = Rc::new(ShutdownState::default());
    let mut driver: Box<dyn PrimaryDriver> = Box::new(LocalDriver::new(Rc::clone(&state)));

    driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap();

    assert_eq!(state.drop_calls.get(), 0);
    drop(driver);
    assert_eq!(state.drop_calls.get(), 1);
}

#[test]
fn cycle_contains_only_the_wait_budget() {
    let cycle = Cycle::new(Duration::from_millis(17));

    assert_eq!(cycle.max_wait(), Duration::from_millis(17));
    assert_eq!(format!("{cycle:?}"), "Cycle { max_wait: 17ms }");
}

#[test]
fn driver_exposes_its_native_waker() {
    let mut driver = LocalDriver::new(Rc::default());
    driver.interrupt = Waker::from(Arc::new(WakeCounter::default()));
    let expected = driver.interrupt.clone();

    assert!(driver.waker().will_wake(&expected));
}

#[test]
fn driver_waker_remains_valid_after_drop() {
    let count = Arc::new(WakeCounter::default());
    let mut driver = LocalDriver::new(Rc::default());
    driver.interrupt = Waker::from(Arc::clone(&count));
    let waker = driver.waker();

    drop(driver);
    waker.wake_by_ref();

    assert_eq!(count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn provider_creation_uses_both_options() {
    fn provider_for<C: IoContext>(options: ProviderOptions) -> C::Provider {
        C::provider(options)
    }

    let provider: TestProvider = provider_for::<TestContext>(ProviderOptions::new());
    let creation = provider.create(driver_options()).unwrap();
    let DriverInstance::Secondary { driver, context } = creation else {
        panic!("test provider must create a secondary instance");
    };

    assert_eq!(context, TestContext(7));
    driver.shutdown().unwrap();
}

#[test]
fn primary_driver_instance_uses_primary_driver() {
    let DriverInstance::Primary { driver, context } =
        DriverInstance::<LocalDriver, LocalDriver, TestContext>::primary(LocalDriver::new(Rc::default()), TestContext(7))
    else {
        panic!("primary constructor must create a primary instance");
    };

    assert_eq!(context, TestContext(7));
    driver.shutdown().unwrap();
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
    let DriverInstance::Secondary { driver, context } = LeaseProvider.create(driver_options()).unwrap() else {
        panic!("lease provider must create a secondary instance");
    };
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
    let DriverInstance::Secondary { driver, context } = LeaseProvider.create(driver_options()).unwrap() else {
        panic!("lease provider must create a secondary instance");
    };

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
fn completion_cycle_shares_wait_budget() {
    let mut first_driver = LocalDriver::new(Rc::default());
    let mut second_driver = LocalDriver::new(Rc::default());
    let mut cycle = Cycle::new(Duration::from_millis(17));

    first_driver.execute_cycle(&mut cycle).unwrap();
    second_driver.execute_cycle(&mut cycle).unwrap();

    assert_eq!(first_driver.cycle_wait, Some(Duration::from_millis(17)));
    assert_eq!(second_driver.cycle_wait, Some(Duration::from_millis(17)));
}

#[derive(Debug, Default)]
struct ShutdownState {
    shutdown_calls: Cell<usize>,
    drop_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    cycle_wait: Option<Duration>,
    interrupt: Waker,
    owned_resource: Option<Box<()>>,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self {
            state,
            cycle_wait: None,
            interrupt: Waker::noop().clone(),
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
    fn shutdown(mut self) -> Result<(), ShutdownError> {
        let _ = self.owned_resource.take();
        self.state.shutdown_calls.update(|calls| calls + 1);
        Ok(())
    }
}

impl PrimaryDriver for LocalDriver {
    fn waker(&self) -> Waker {
        self.interrupt.clone()
    }

    fn execute_cycle(&mut self, cycle: &mut Cycle) -> Result<(), DriverError> {
        self.cycle_wait = Some(cycle.max_wait());
        Ok(())
    }
}

impl SecondaryDriver for LocalDriver {}

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
    type Context = TestContext;
    type Primary = LocalDriver;
    type Secondary = LocalDriver;

    fn create(self, _options: DriverOptions) -> Result<DriverInstance<Self::Primary, Self::Secondary, Self::Context>, DriverError> {
        Ok(DriverInstance::secondary(LocalDriver::new(Rc::default()), TestContext(7)))
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
    type Context = LeaseContext;
    type Primary = LeaseDriver;
    type Secondary = LeaseDriver;

    fn create(self, _options: DriverOptions) -> Result<DriverInstance<Self::Primary, Self::Secondary, Self::Context>, DriverError> {
        let state = Arc::default();
        Ok(DriverInstance::secondary(
            LeaseDriver::new(Arc::clone(&state)),
            LeaseContext { state },
        ))
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

impl PrimaryDriver for LeaseDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }
}

impl SecondaryDriver for LeaseDriver {}

fn driver_options() -> DriverOptions {
    DriverOptions::new(
        worker_thread(),
        SystemTaskSpawner::from_fn(|task| task()),
        vec![DriverRole::Secondary],
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
