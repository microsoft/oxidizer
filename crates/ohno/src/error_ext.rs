// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::Backtrace as StdBacktrace;
use std::error::Error as StdError;

/// Extension trait providing additional functionality for ohno error types.
///
/// This trait is automatically implemented by `#[derive(Error)]` and `#[ohno::error]`.
/// It provides convenient methods for error handling, backtrace access, and error chain traversal.
///
/// # Examples
///
/// ```rust
/// use ohno::ErrorExt;
///
/// #[ohno::error]
/// #[from(std::io::Error)]
/// struct NetworkError;
///
/// #[ohno::error]
/// #[from(NetworkError)]
/// struct ServiceError;
///
/// let io_error = std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "connection aborted");
/// let network_err = NetworkError::from(io_error);
/// let service_err = ServiceError::from(network_err);
/// let io_error = service_err.find_source::<std::io::Error>().unwrap();
/// ```
pub trait ErrorExt: StdError {
    /// Returns the formatted error message without backtrace.
    ///
    /// Provides a clean, user-friendly error message excluding backtrace information.
    /// Ideal for user interfaces, logs, or when backtrace details are not needed.
    fn message(&self) -> String;

    /// Returns a reference to the captured backtrace.
    ///
    /// Provides access to the stack trace captured when the error was created.
    /// Use [`has_backtrace()`](Self::has_backtrace) to check if backtrace was captured.
    ///
    /// # Backtrace Capture
    ///
    /// Controlled by environment variables:
    /// - `RUST_BACKTRACE=1` enables basic backtrace
    /// - `RUST_BACKTRACE=full` enables full backtrace with all frames
    fn backtrace(&self) -> &StdBacktrace;

    /// Returns `true` if the error has a captured backtrace.
    ///
    /// Convenience method equivalent to checking if backtrace status is [`Captured`](std::backtrace::BacktraceStatus::Captured).
    fn has_backtrace(&self) -> bool {
        self.backtrace().status() == std::backtrace::BacktraceStatus::Captured
    }

    /// Finds the first source error of the specified type in the error chain.
    ///
    /// Walks through the error's source chain and returns the first error that matches type `T`.
    /// Only searches the **source chain**, not the current error itself.
    fn find_source<T: StdError + 'static>(&self) -> Option<&T> {
        self.find_source_with(|_| true)
    }

    /// Finds the first source error of the specified type that matches the given predicate.
    ///
    /// Walks through the error's source chain and returns the first error that matches type `T`
    /// and satisfies the provided search predicate. Only searches the **source chain**, not the
    /// current error itself.
    fn find_source_with<T: StdError + 'static>(&self, search: impl Fn(&T) -> bool) -> Option<&T> {
        let mut source = self.source();
        while let Some(err) = source {
            if let Some(target) = err.downcast_ref::<T>()
                && search(target)
            {
                return Some(target);
            }
            source = err.source();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::backtrace::BacktraceStatus;

    use super::*;
    use crate::backtrace::Backtrace;

    #[ohno::error]
    struct TestError;

    #[cfg_attr(miri, ignore)] // unsupported operation: `GetCurrentDirectoryW` not available when isolation is enabled
    #[test]
    fn force_backtrace_capture() {
        let mut error = TestError::new();
        error.0.data.backtrace = Backtrace::force_capture();

        assert!(error.has_backtrace());
        let backtrace = error.backtrace();
        assert!(backtrace.status() == BacktraceStatus::Captured);
        let display = format!("{error}");
        assert!(display.starts_with("TestError\n\nBacktrace:\n"));
    }

    #[test]
    fn no_backtrace_capture() {
        let mut error = TestError::new();
        error.0.data.backtrace = Backtrace::disabled();
        assert!(!error.has_backtrace());
        assert!(error.backtrace().status() == BacktraceStatus::Disabled);
        let display = format!("{error}");
        assert_eq!(display, "TestError");
    }
}
