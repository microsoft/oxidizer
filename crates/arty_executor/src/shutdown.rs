// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::fmt::{self, Debug};

use ohno::OhnoCore;

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

impl ShutdownCause {
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

    use static_assertions::assert_impl_all;

    use super::{ShutdownCause, ShutdownError};

    #[derive(Debug)]
    struct SourceError;

    impl Display for SourceError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("source failure")
        }
    }

    impl Error for SourceError {}

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
