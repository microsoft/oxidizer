// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Waker;

use arty_executor::{CycleOutcome, Executor, Shutdown, ShutdownCause, ShutdownError};
use static_assertions::{assert_impl_all, assert_not_impl_any};

#[derive(Debug)]
struct SourceError;

impl Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("source failure")
    }
}

impl Error for SourceError {}

struct WakeCount(AtomicUsize);

impl std::task::Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        // Trigger calls the waker synchronously before the test reads the counter.
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn shutdown_completion_requires_a_shutdown_request() {
    let mut executor = Executor::builder().build();

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    executor.shutdown().trigger(ShutdownCause::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
}

#[test]
fn shutdown_handle_is_cloneable_and_send_only() {
    assert_impl_all!(Shutdown: Clone, Debug, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(Shutdown: Send, Sync);
}

#[test]
fn first_trigger_publishes_the_cause_and_wakes_the_owner_once() {
    let wake_count = Arc::new(WakeCount(AtomicUsize::new(0)));
    let executor = Executor::builder().owner_waker(Waker::from(Arc::clone(&wake_count))).build();
    let shutdown = executor.shutdown();
    let expected = ShutdownError::new(SourceError);

    shutdown.trigger(expected.clone().into());
    shutdown.trigger(ShutdownCause::default());

    let actual = shutdown.cause().into_error().unwrap();
    assert!(std::ptr::eq(
        actual.inner().downcast_ref::<SourceError>().unwrap(),
        expected.inner().downcast_ref::<SourceError>().unwrap()
    ));
    assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn default_cause_has_no_error() {
    assert_impl_all!(ShutdownCause: Clone, Debug, Default, Send, Sync, UnwindSafe, RefUnwindSafe);

    assert!(ShutdownCause::default().as_error().is_none());
}

#[test]
fn default_cause_and_clone_have_no_error() {
    let cause = ShutdownCause::default();
    let clone = cause.clone();

    assert!(cause.as_error().is_none());
    assert!(clone.as_error().is_none());
}

#[test]
fn panic_cause_preserves_its_diagnostic() {
    let cause = ShutdownCause::panic(&());

    assert_eq!(
        cause.as_error().unwrap().inner().to_string(),
        "runtime shutdown was caused by a task panic: non-string panic payload"
    );
}

#[test]
fn cloned_error_preserves_source_identity() {
    assert_impl_all!(ShutdownError: Clone, Debug, Display, Error, Send, Sync, UnwindSafe, RefUnwindSafe);

    let error = ShutdownError::new(SourceError);
    let clone = error.clone();

    let source = error.inner().downcast_ref::<SourceError>().unwrap();
    let cloned_source = clone.inner().downcast_ref::<SourceError>().unwrap();
    assert!(std::ptr::eq(source, cloned_source));
    assert_eq!(error.to_string(), "source failure");
    assert!(format!("{error:?}").contains("ShutdownError"));
}

#[test]
fn consuming_error_cause_returns_the_shutdown_error() {
    let error = ShutdownError::new(SourceError);
    let expected = error.clone();

    let consumed = ShutdownCause::from_error(error).into_error().unwrap();

    let expected = expected.inner().downcast_ref::<SourceError>().unwrap();
    let consumed = consumed.inner().downcast_ref::<SourceError>().unwrap();
    assert!(std::ptr::eq(expected, consumed));
}
