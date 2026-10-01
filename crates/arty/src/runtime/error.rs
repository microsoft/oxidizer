// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;
use std::fmt::{self, Display};

/// An error constructing or operating a runtime.
///
/// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns this
/// error when a processor count is zero or a policy cannot be satisfied, such
/// as an exact count exceeding the available processors.
///
/// [`RuntimeOperations::pin_to`](crate::runtime::RuntimeOperations::pin_to)
/// returns this error for a worker coordinate that cannot provide affinity
/// information for its runtime.
///
/// [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on)
/// returns this error for an invalid calling context or a failed task. A task
/// failure is retained as a [`JoinError`](crate::task::JoinError) source.
///
/// Format the error with [`Display`] and inspect [`StdError::source`] for
/// diagnostics. Construction and affinity error messages and concrete source
/// types are not stable error classifications. Task joins report
/// [`JoinError`](crate::task::JoinError) directly.
#[derive(Debug)]
pub struct Error {
    source: Box<dyn StdError + Send + Sync>,
}

impl Error {
    pub(crate) fn new(source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self { source: source.into() }
    }

    pub(crate) fn insufficient_processors(requested: usize, available: usize) -> Self {
        Self::new(InsufficientProcessors { requested, available })
    }

    #[cfg(any(test, feature = "macros"))]
    pub(crate) fn into_source(self) -> Box<dyn StdError + Send + Sync> {
        self.source
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.source.as_ref(), f)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

#[derive(Debug)]
struct InsufficientProcessors {
    requested: usize,
    available: usize,
}

impl Display for InsufficientProcessors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "requested {} processors, but only {} are available",
            self.requested, self.available
        )
    }
}

impl StdError for InsufficientProcessors {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn processor_failure_preserves_requested_and_available_counts() {
        let error = Error::insufficient_processors(17, 3);
        assert_eq!(error.to_string(), "requested 17 processors, but only 3 are available");
        assert!(error.source().unwrap().source().is_none());
    }

    #[test]
    fn internal_constructor_preserves_display_context() {
        let error = Error::new(io::Error::other("thread creation context"));
        assert!(error.to_string().contains("thread creation context"));
    }

    #[test]
    fn internal_constructor_preserves_typed_source() {
        let error = Error::new(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(
            error.source().unwrap().downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn internal_constructor_preserves_boxed_source() {
        let source: Box<dyn StdError + Send + Sync> = Box::new(io::Error::from(io::ErrorKind::TimedOut));
        let error = Error::new(source);
        assert_eq!(
            error.source().unwrap().downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
