// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::cell::OnceCell;
use std::error::Error;
use std::fmt::{self, Debug};
use std::rc::Rc;
use std::sync::Arc;
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

    /// Returns whether executor shutdown has started.
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.inner.cause.get().is_some()
    }

    /// Returns the first shutdown cause.
    ///
    /// Returns the default cause if shutdown has not started.
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

struct ShutdownInner {
    cause: OnceCell<ShutdownCause>,
    owner_waker: Waker,
}

/// A cloneable error that caused executor shutdown.
///
/// Clones share the same immutable source error. Use [`inner`](Self::inner) or
/// [`Error::source`] to inspect the original error and its source chain.
/// Debug output includes the original error and any panic message.
#[derive(Clone, derive_more::Debug)]
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
    /// Returns whether a contained task panic initiated shutdown.
    #[must_use]
    pub fn is_panic(&self) -> bool {
        self.inner().is::<ContainedRuntimePanic>()
    }

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

impl ShutdownCause {
    /// Creates a shutdown cause without consuming the original panic payload.
    ///
    /// String payloads supply the diagnostic message. Other payloads use
    /// `non-string panic payload` because they have no required display format.
    #[must_use]
    pub fn panic(payload: &(dyn Any + Send)) -> Self {
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        ShutdownError::new(ContainedRuntimePanic {
            message: Arc::from(message),
        })
        .into()
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

#[derive(Debug)]
struct ContainedRuntimePanic {
    message: Arc<str>,
}

impl fmt::Display for ContainedRuntimePanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime shutdown was caused by a task panic: {}", self.message)
    }
}

impl Error for ContainedRuntimePanic {}

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
