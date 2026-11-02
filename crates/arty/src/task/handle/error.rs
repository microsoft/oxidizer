// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::error::Error;
use std::fmt::{self, Debug, Display};

use arty_executor::ShutdownError;
use performables::sync::mutex::Mutex;

/// A task failed to return its result.
///
/// Returned by [`JoinHandle`](super::JoinHandle) and as the underlying error of
/// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on).
/// [`is_panic`](Self::is_panic) identifies a task panic.
/// [`is_shutdown`](Self::is_shutdown) identifies cancellation or
/// a task rejected during shutdown.
/// [`is_invalid_destination`](Self::is_invalid_destination)
/// identifies a requested worker that does not belong to this runtime.
///
/// This is separate from an error returned by the task's own code. Awaiting a
/// task that returns `Result<T, E>` produces `Result<Result<T, E>, JoinError>`.
/// Receiving a panic error does not restart the panic or repair application data.
/// By default, a task panic that Arty catches also starts runtime shutdown.
/// [`PanicPolicy::ignore`](crate::runtime::PanicPolicy::ignore) keeps the
/// runtime running while `JoinHandle` still reports the panic.
/// With `panic = "abort"`, a task panic terminates the process instead.
///
/// If a runtime failure caused shutdown, [`Error::source`] returns that
/// failure. If shutdown was requested without an error, `source()` returns `None`.
///
/// # Examples
///
/// Starting a task after shutdown returns a task error without calling its factory:
///
/// ```
/// use arty::runtime::Runtime;
///
/// let runtime = Runtime::new()?;
/// let error = runtime.scheduler().block_on(async |cx| {
///     cx.control().request_stop();
///     cx.scheduler()
///         .spawn(async |_| 42)
///         .await
///         .expect_err("submission follows shutdown")
/// })?;
/// assert!(error.is_shutdown());
/// assert!(!error.is_panic());
/// runtime.stop()?;
/// # Ok::<(), arty::runtime::Error>(())
/// ```
pub struct JoinError {
    kind: Kind,
}

enum Kind {
    // The panic value can be Send without being Sync. It is consumed, never borrowed.
    Panic(Mutex<Box<dyn Any + Send + 'static>>),
    Shutdown(Option<ShutdownError>),
    InvalidDestination,
}

impl JoinError {
    /// Returns `true` if the task panicked.
    ///
    /// With `panic = "unwind"`, Arty can catch panics instead of terminating
    /// the process. This includes panics in a task factory, an async future,
    /// or a blocking function.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "macros", feature = "rt"))]
    /// # #[arty::main(panic = ignore)]
    /// # async fn main(cx: arty::task::Context) -> Result<(), arty::task::JoinError> {
    /// let result: Result<(), arty::task::JoinError> =
    ///     cx.scheduler().spawn(async |_| panic!("task failed")).await;
    /// let error = result.expect_err("the task deliberately panics");
    /// assert!(error.is_panic());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
    /// ```
    #[must_use]
    pub const fn is_panic(&self) -> bool {
        matches!(&self.kind, Kind::Panic(_))
    }

    /// Returns `true` if shutdown cancelled the task or prevented it from starting.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::new()?;
    /// let error = runtime.scheduler().block_on(async |cx| {
    ///     cx.control().request_stop();
    ///     cx.scheduler()
    ///         .spawn(async |_| 42)
    ///         .await
    ///         .expect_err("submission follows shutdown")
    /// })?;
    /// assert!(error.is_shutdown());
    /// runtime.stop()?;
    /// # Ok::<(), arty::runtime::Error>(())
    /// ```
    #[must_use]
    pub const fn is_shutdown(&self) -> bool {
        matches!(&self.kind, Kind::Shutdown(_))
    }

    /// Returns `true` if the requested destination is not a worker of the runtime.
    #[must_use]
    pub const fn is_invalid_destination(&self) -> bool {
        matches!(&self.kind, Kind::InvalidDestination)
    }

    pub(crate) fn panicked(payload: Box<dyn Any + Send + 'static>) -> Self {
        Self {
            kind: Kind::Panic(Mutex::new(payload)),
        }
    }

    pub(crate) const fn shutdown(error: Option<ShutdownError>) -> Self {
        Self {
            kind: Kind::Shutdown(error),
        }
    }

    pub(crate) const fn invalid_destination() -> Self {
        Self {
            kind: Kind::InvalidDestination,
        }
    }

    /// Returns the value carried by the panic, or `None` for other failures.
    #[must_use]
    pub fn into_panic(self) -> Option<Box<dyn Any + Send + 'static>> {
        match self.kind {
            Kind::Panic(payload) => Some(payload.into_inner()),
            Kind::Shutdown(_) | Kind::InvalidDestination => None,
        }
    }
}

impl Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("is_panic", &self.is_panic())
            .field("is_shutdown", &self.is_shutdown())
            .field("is_invalid_destination", &self.is_invalid_destination())
            .finish()
    }
}

impl Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match &self.kind {
            Kind::Panic(_) => "task panicked",
            Kind::Shutdown(_) => "runtime is shutting down",
            Kind::InvalidDestination => "target thread is not a worker of this runtime",
        })
    }
}

impl Error for JoinError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            Kind::Shutdown(Some(error)) => Some(error),
            Kind::Panic(_) | Kind::Shutdown(None) | Kind::InvalidDestination => None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn failure_kinds_are_distinct_and_thread_safe() {
        static_assertions::assert_impl_all!(JoinError: Error, Send, Sync);
        let panic = JoinError::panicked(Box::new(std::cell::Cell::new(42)));
        assert!(panic.is_panic());
        assert!(!panic.is_shutdown());
        assert!(!panic.is_invalid_destination());
        assert_eq!(panic.to_string(), "task panicked");

        let shutdown = JoinError::shutdown(None);
        assert!(!shutdown.is_panic());
        assert!(shutdown.is_shutdown());
        assert!(!shutdown.is_invalid_destination());
        assert_eq!(shutdown.to_string(), "runtime is shutting down");
        assert!(format!("{shutdown:?}").contains("is_shutdown: true"));
        assert!(shutdown.source().is_none());
    }
}
