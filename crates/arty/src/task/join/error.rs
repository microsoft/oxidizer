// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::error::Error;
use std::fmt::{self, Debug, Display};
use std::sync::Mutex;

/// A task failed to return its result.
///
/// Returned by [`JoinHandle`](super::JoinHandle),
/// [`LocalJoinHandle`](super::LocalJoinHandle), and as the source of a scheduler's
/// `block_on` error. Use [`is_panic`](Self::is_panic) to identify a task panic
/// or [`is_shutdown`](Self::is_shutdown) to identify cancellation or rejection.
///
/// This is separate from an error returned by the task's own code. Joining a
/// task that returns `Result<T, E>` produces `Result<Result<T, E>, JoinError>`.
/// Receiving a panic error does not resume unwinding or repair application state.
/// With `panic = "abort"`, a task panic aborts the process instead.
///
/// # Examples
///
/// A submission after shutdown reports a task error without invoking its factory:
///
/// ```
/// use arty::runtime::{Runtime, RuntimeOperations};
///
/// let runtime = Runtime::new()?;
/// let scheduler = runtime.scheduler();
/// RuntimeOperations::from(&runtime).request_stop();
/// let error = scheduler
///     .spawn_anywhere(async |_| 42)
///     .wait()
///     .expect_err("submission follows shutdown");
/// assert!(error.is_shutdown());
/// assert!(!error.is_panic());
/// runtime.stop()?;
/// # Ok::<(), arty::runtime::Error>(())
/// ```
pub struct JoinError {
    // The payload can be Send without being Sync. It is only consumed, never borrowed.
    panic: Option<Mutex<Box<dyn Any + Send + 'static>>>,
}

impl JoinError {
    /// Returns `true` if the task panicked.
    ///
    /// This includes panics in a factory, an asynchronous future, or a blocking
    /// callback when unwinding is enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "macros")]
    /// #[arty::main]
    /// async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
    ///     let result: Result<(), arty::task::JoinError> =
    ///         cx.scheduler().spawn(async |_| panic!("task failed")).await;
    ///     let error = result.expect_err("the task deliberately panics");
    ///     assert!(error.is_panic());
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "macros"))] fn main() {}
    /// ```
    #[must_use]
    pub const fn is_panic(&self) -> bool {
        self.panic.is_some()
    }

    /// Returns `true` if shutdown cancelled the task or rejected its submission.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, RuntimeOperations};
    ///
    /// let runtime = Runtime::new()?;
    /// let scheduler = runtime.scheduler();
    /// RuntimeOperations::from(&runtime).request_stop();
    /// let error = scheduler
    ///     .spawn_anywhere(async |_| 42)
    ///     .wait()
    ///     .expect_err("submission follows shutdown");
    /// assert!(error.is_shutdown());
    /// runtime.stop()?;
    /// # Ok::<(), arty::runtime::Error>(())
    /// ```
    #[must_use]
    pub const fn is_shutdown(&self) -> bool {
        self.panic.is_none()
    }

    pub(crate) fn panicked(payload: Box<dyn Any + Send + 'static>) -> Self {
        Self {
            panic: Some(Mutex::new(payload)),
        }
    }

    pub(crate) const fn shutdown() -> Self {
        Self { panic: None }
    }

    #[cfg(feature = "macros")]
    pub(crate) fn resume(self) -> ! {
        let payload = match self.panic {
            Some(payload) => payload.into_inner().expect("the panic payload mutex is never locked"),
            None => Box::new("runtime is shutting down"),
        };
        std::panic::resume_unwind(payload)
    }
}

impl Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinError")
            .field("is_panic", &self.is_panic())
            .field("is_shutdown", &self.is_shutdown())
            .finish()
    }
}

impl Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_panic() {
            "task panicked"
        } else {
            "runtime is shutting down"
        })
    }
}

impl Error for JoinError {}

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
        assert_eq!(panic.to_string(), "task panicked");

        let shutdown = JoinError::shutdown();
        assert!(!shutdown.is_panic());
        assert!(shutdown.is_shutdown());
        assert_eq!(shutdown.to_string(), "runtime is shutting down");
        assert!(format!("{shutdown:?}").contains("is_shutdown: true"));
        assert!(shutdown.source().is_none());
    }
}
