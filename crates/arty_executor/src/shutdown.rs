// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::OnceCell;
use std::error::Error;
use std::fmt::{self, Debug};
use std::rc::Rc;
use std::task::Waker;

use ohno::OhnoCore;

/// A handle that requests executor shutdown.
///
/// Clones share the first shutdown cause on the executor owner thread.
#[derive(Clone, derive_more::Debug)]
pub struct Shutdown {
    #[debug(ignore)]
    inner: Rc<ShutdownInner>,
}

// Shutdown records a complete, write-once cause before it invokes the owner
// waker. A panicking waker cannot expose a partial cause through another clone.
impl std::panic::UnwindSafe for Shutdown {}
impl std::panic::RefUnwindSafe for Shutdown {}

struct ShutdownInner {
    cause: OnceCell<ShutdownCause>,
    owner_waker: Waker,
}

impl Shutdown {
    pub(crate) fn new(owner_waker: Waker) -> Self {
        Self {
            inner: Rc::new(ShutdownInner {
                cause: OnceCell::new(),
                owner_waker,
            }),
        }
    }

    /// Requests executor shutdown with `cause`.
    ///
    /// The first cause remains authoritative. Later requests do not replace it.
    ///
    /// # Panics
    ///
    /// Panics if the executor owner waker panics.
    pub fn trigger(&self, cause: ShutdownCause) {
        if self.publish(cause) {
            self.inner.owner_waker.wake_by_ref();
        }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.inner.cause.get().is_some()
    }

    #[doc(hidden)]
    #[must_use]
    pub fn cause(&self) -> ShutdownCause {
        self.inner.cause.get().cloned().unwrap_or_default()
    }

    /// Records the first shutdown cause without waking the executor owner.
    ///
    /// Returns `true` only for the caller that records the cause. The executor
    /// uses this while it is already running a cycle and during synchronous
    /// cleanup, when another owner notification is not needed.
    pub(crate) fn publish(&self, cause: ShutdownCause) -> bool {
        self.inner.cause.set(cause).is_ok()
    }
}

/// A cloneable error that caused executor shutdown.
///
/// Clones share the same immutable source error. Use [`inner`](Self::inner) or
/// [`Error::source`] to inspect the original error and its source chain.
#[derive(Clone)]
pub struct ShutdownError {
    inner: ShutdownErrorInner,
}

#[derive(ohno::Error, Clone)]
#[no_constructors]
struct ShutdownErrorInner {
    #[error]
    core: OhnoCore,
}

// The shared source is immutable through this wrapper. Source implementations
// must already satisfy Send + Sync, and ShutdownError adds no mutable state.
impl std::panic::UnwindSafe for ShutdownError {}
impl std::panic::RefUnwindSafe for ShutdownError {}

impl ShutdownError {
    /// Wraps an error that caused shutdown.
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self {
            inner: ShutdownErrorInner {
                core: OhnoCore::without_backtrace(error),
            },
        }
    }

    /// Returns the original error that caused shutdown.
    ///
    /// # Panics
    ///
    /// Panics only if an internal construction invariant is broken and the
    /// error no longer contains the source supplied to [`new`](Self::new).
    #[must_use]
    pub fn inner(&self) -> &(dyn Error + 'static) {
        self.inner
            .core
            .source()
            .expect("ShutdownError construction always records a source error")
    }
}

impl Debug for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShutdownError").finish_non_exhaustive()
    }
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl Error for ShutdownError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.inner())
    }
}

/// Describes why executor shutdown started.
///
/// The default value represents shutdown without an error.
#[derive(Clone, Default)]
pub struct ShutdownCause {
    error: Option<ShutdownError>,
}

#[ohno::error]
#[display("a contained runtime operation panicked")]
struct ContainedRuntimePanic;

impl ShutdownCause {
    /// Creates a shutdown cause for a contained runtime panic.
    #[must_use]
    pub fn panic() -> Self {
        ShutdownError::new(ContainedRuntimePanic::new()).into()
    }

    /// Creates a shutdown cause from an error.
    #[must_use]
    pub fn from_error(error: ShutdownError) -> Self {
        Self { error: Some(error) }
    }

    /// Returns the shutdown error, if shutdown started because of an error.
    #[must_use]
    pub const fn as_error(&self) -> Option<&ShutdownError> {
        self.error.as_ref()
    }

    /// Returns the shutdown error, if shutdown started because of an error.
    #[must_use]
    pub fn into_error(self) -> Option<ShutdownError> {
        self.error
    }
}

impl From<ShutdownError> for ShutdownCause {
    fn from(error: ShutdownError) -> Self {
        Self::from_error(error)
    }
}

impl Debug for ShutdownCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShutdownCause")
            .field("has_error", &self.as_error().is_some())
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::{self, Debug, Display};
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{Shutdown, ShutdownCause, ShutdownError};

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
            // Trigger calls the waker synchronously, and the test reads the
            // counter after both calls. No data is published through it.
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn shutdown_handle_is_cloneable_and_send_only() {
        assert_impl_all!(Shutdown: Clone, Debug, UnwindSafe, RefUnwindSafe);
        assert_not_impl_any!(Shutdown: Send, Sync);
    }

    #[test]
    fn first_trigger_publishes_the_cause_and_wakes_the_owner_once() {
        let wake_count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let shutdown = Shutdown::new(Waker::from(Arc::clone(&wake_count)));
        let expected = ShutdownError::new(SourceError);

        shutdown.trigger(expected.clone().into());
        shutdown.trigger(ShutdownCause::default());

        let actual = shutdown.cause().into_error().unwrap();
        assert!(std::ptr::eq(
            actual.inner().downcast_ref::<SourceError>().unwrap(),
            expected.inner().downcast_ref::<SourceError>().unwrap()
        ));
        // The synchronous trigger calls complete before this load.
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
        let cause = ShutdownCause::panic();

        assert_eq!(
            cause.as_error().unwrap().inner().to_string(),
            "a contained runtime operation panicked"
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
}
