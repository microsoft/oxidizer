// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public surface contract tests.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::cell::Cell;
use std::error::Error;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Wake, Waker};
use std::time::Duration;
use std::{fmt, thread};

use arty_io_core::{Cycle, Driver, DriverError, DriverOptions, DriverRole, IoContext};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware_core::{Thread, ThreadAware};

assert_impl_all!(DriverOptions: Send, Sync, fmt::Debug);
assert_not_impl_any!(Cycle: Send, Sync);
assert_impl_all!(DriverRole: Copy, Send, Sync, fmt::Debug, fmt::Display, Eq);
assert_impl_all!(DriverError: Send, Sync, fmt::Debug, fmt::Display, Error);

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn driver_options_preserve_runtime_inputs() {
    let worker = worker_thread();
    let wake_counter = Arc::new(WakeCounter::default());
    let owner_waker = Waker::from(Arc::clone(&wake_counter));
    let options = DriverOptions::new(worker.clone(), owner_waker, DriverRole::Primary);

    options.owner_waker().wake_by_ref();

    assert_eq!(
        (options.thread(), options.role(), wake_counter.0.load(Ordering::Relaxed),),
        (&worker, DriverRole::Primary, 1,)
    );
}

#[test]
fn driver_can_be_thread_local_and_type_erased() {
    assert_not_impl_any!(LocalDriver: Send, Sync);

    fn assert_driver<T: Driver>() {}
    assert_driver::<LocalDriver>();

    let mut driver: Box<dyn Driver> = Box::new(LocalDriver::default());
    driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap();
}

#[test]
fn cycle_preserves_max_wait() {
    let max_wait = Duration::from_millis(17);
    assert_eq!(Cycle::new(max_wait).max_wait(), max_wait);
}

#[test]
fn driver_roles_have_stable_labels() {
    assert_eq!(
        (DriverRole::Primary.to_string(), DriverRole::Secondary.to_string()),
        ("primary".to_owned(), "secondary".to_owned())
    );
}

#[test]
fn context_creates_driver_with_selected_role() {
    let mut context = TestContext::context(7);
    let _driver = context.driver(driver_options()).unwrap();

    assert_eq!(context, TestContext(7));
}

#[test]
fn shutdown_consumes_the_driver() {
    let shutdown_calls = Rc::new(Cell::new(0));
    let driver = LocalDriver {
        shutdown_calls: Rc::clone(&shutdown_calls),
    };

    driver.shutdown().unwrap();

    assert_eq!(shutdown_calls.get(), 1);
}

#[test]
fn driver_error_preserves_message_and_source() {
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
struct LocalDriver {
    shutdown_calls: Rc<Cell<usize>>,
}

impl Driver for LocalDriver {
    fn waker(&self) -> Waker {
        Waker::noop().clone()
    }

    fn execute_cycle(&mut self, _cycle: &mut Cycle) -> Result<(), DriverError> {
        Ok(())
    }

    fn shutdown(self) -> Result<(), DriverError> {
        self.shutdown_calls.update(|calls| calls + 1);
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

    fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
        Ok(LocalDriver::default())
    }
}

fn driver_options() -> DriverOptions {
    DriverOptions::new(worker_thread(), Waker::noop().clone(), DriverRole::Primary)
}

fn worker_thread() -> Thread {
    let owner = thread_aware_core::__private::v1::new_owner();
    let numa_node = thread_aware_core::__private::v1::new_numa_node(0);
    thread_aware_core::__private::v1::new_thread(owner, thread::current().id(), numa_node)
}
