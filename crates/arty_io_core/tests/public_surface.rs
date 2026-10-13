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
use std::time::Duration;
use std::{fmt, thread};

use arty_io_core::{Cycle, Driver, DriverError, DriverInstance, DriverOptions, DriverRole, IoContext};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use testing_aids::TEST_TIMEOUT;
use thread_aware_core::{Thread, ThreadAware};

assert_impl_all!(DriverOptions: Send, Sync, fmt::Debug);
assert_not_impl_any!(Cycle: Send, Sync);
assert_impl_all!(DriverRole: Copy, Send, Sync, fmt::Debug, Eq);
assert_impl_all!(DriverError: Send, Sync, fmt::Debug, fmt::Display, Error);

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn public_options_expose_runtime_facilities() {
    let worker = worker_thread();
    let wake_counter = Arc::new(WakeCounter::default());
    let owner_waker = Waker::from(Arc::clone(&wake_counter));
    let options = DriverOptions::new(worker.clone(), owner_waker, vec![DriverRole::Primary, DriverRole::Secondary]);

    options.owner_waker().wake_by_ref();

    assert_eq!(options.thread(), &worker);
    assert_eq!(options.allowed_roles(), [DriverRole::Primary, DriverRole::Secondary]);
    assert_eq!(wake_counter.0.load(Ordering::Relaxed), 1);
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
    let mut driver: Box<dyn Driver> = Box::new(LocalDriver::new(Rc::clone(&state)));

    driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap();

    assert_eq!(state.drop_calls.get(), 0);
    drop(driver);
    assert_eq!(state.drop_calls.get(), 1);
}

#[test]
fn cycle_exposes_max_wait() {
    // Any non-default duration proves that Cycle preserves the supplied value.
    let cycle = Cycle::new(Duration::from_millis(17));
    assert_eq!(cycle.max_wait(), Duration::from_millis(17));
}

#[test]
fn context_and_driver_creation_use_both_options() {
    fn context<C: IoContext>(options: C::ContextOptions) -> C {
        C::context(options)
    }

    // Any non-default option proves that IoContext::context receives the caller's value.
    let mut context: TestContext = context::<TestContext>(7);
    let instance = context.driver(driver_options()).unwrap();
    assert_eq!(instance.role, DriverRole::Primary);
    let role = instance.role;
    let driver: LocalDriver = instance.driver;

    assert_eq!(context, TestContext(7));
    assert_eq!(role, DriverRole::Primary);
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
    let mut context = LeaseContext::context(());
    let driver = context.driver(driver_options()).unwrap().driver;
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

    shutdown_rx.recv_timeout(TEST_TIMEOUT).unwrap().unwrap();
    shutdown_thread.join().unwrap();
}

#[test]
fn dropping_driver_closes_context_admission() {
    let mut context = LeaseContext::context(());
    let driver = context.driver(driver_options()).unwrap().driver;

    drop(driver);

    assert!(context.begin_operation().is_none());
}

#[test]
fn driver_error_can_be_created_from_message_and_source() {
    #[derive(Debug)]
    struct SentinelError;

    impl fmt::Display for SentinelError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("sentinel")
        }
    }

    impl Error for SentinelError {}

    let error = DriverError::from_message("native driver failed");
    assert_eq!(error.to_string(), "native driver failed");
    assert!(error.source().is_none());

    let error = DriverError::from_source(SentinelError);
    assert_eq!(error.to_string(), "sentinel");
    assert!(error.source().is_some_and(<dyn Error>::is::<SentinelError>));
}

#[derive(Debug, Default)]
struct ShutdownState {
    shutdown_calls: Cell<usize>,
    drop_calls: Cell<usize>,
}

#[derive(Debug)]
struct LocalDriver {
    state: Rc<ShutdownState>,
    owned_resource: Option<Box<()>>,
}

impl LocalDriver {
    fn new(state: Rc<ShutdownState>) -> Self {
        Self {
            state,
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
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(mut self) -> Result<(), DriverError> {
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
    type ContextOptions = u8;
    type Driver = LocalDriver;

    fn context(options: Self::ContextOptions) -> Self {
        Self(options)
    }

    fn driver(&mut self, _options: DriverOptions) -> Result<DriverInstance<Self::Driver>, DriverError> {
        Ok(DriverInstance::new(LocalDriver::new(Rc::default()), DriverRole::Primary))
    }
}

/// Application handle. Cloning or retaining it does not count as an active operation.
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
    type ContextOptions = ();
    type Driver = LeaseDriver;

    fn context((): Self::ContextOptions) -> Self {
        let state = Arc::default();
        Self { state }
    }

    fn driver(&mut self, _options: DriverOptions) -> Result<DriverInstance<Self::Driver>, DriverError> {
        Ok(DriverInstance::new(
            LeaseDriver::new(Arc::clone(&self.state)),
            DriverRole::Secondary,
        ))
    }
}

/// Shared lock and notification state used by the context, operation leases, and driver.
#[derive(Debug, Default)]
struct LeaseState {
    lifecycle: Mutex<LeaseLifecycle>,
    changed: Condvar,
}

/// Admission state and active-operation count protected by `LeaseState::lifecycle`.
#[derive(Debug, Default)]
struct LeaseLifecycle {
    shutdown_started: bool,
    active_operations: usize,
}

/// Counted ownership of one operation that must finish before shutdown returns successfully.
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

/// Driver that closes admission and waits for all `OperationLease` values to be dropped.
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
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        let mut lifecycle = self.state.lifecycle.lock().unwrap_or_else(PoisonError::into_inner);
        lifecycle.shutdown_started = true;
        self.state.changed.notify_all();

        while lifecycle.active_operations != 0 {
            lifecycle = self.state.changed.wait(lifecycle).unwrap_or_else(PoisonError::into_inner);
        }

        assert_eq!(
            lifecycle.active_operations, 0,
            "successful shutdown requires all operations to drain"
        );
        Ok(())
    }
}

fn driver_options() -> DriverOptions {
    DriverOptions::new(
        worker_thread(),
        Waker::noop().clone(),
        vec![DriverRole::Primary, DriverRole::Secondary],
    )
}

fn worker_thread() -> Thread {
    let owner = thread_aware_core::__private::v1::new_owner();
    let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
    thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node)
}
