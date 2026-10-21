// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;
use std::fmt::{self, Display};

use arty_io_core::DriverError;
use performables::arc::Arc;

/// An error constructing or operating a runtime.
///
/// You may receive this error when building a runtime with an invalid worker
/// count or blocking-pool limit, requesting more workers than are available,
/// waiting from an async worker, or stopping a runtime whose worker or I/O
/// driver failed.
///
/// When [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on)
/// fails because a task panicked or was cancelled, its source is a
/// [`JoinError`](crate::task::JoinError).
///
/// Format the error with [`Display`] and inspect [`StdError::source`] for
/// diagnostics. Do not depend on specific messages or source types to classify
/// construction or affinity failures. Task joins report `JoinError` directly.
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

    pub(crate) fn invalid_worker_count() -> Self {
        Self::new(RuntimeValidation::InvalidWorkerCount)
    }

    pub(crate) fn invalid_blocking_pool_limit() -> Self {
        Self::new(RuntimeValidation::InvalidBlockingPoolLimit)
    }

    pub(crate) fn block_on_from_worker() -> Self {
        Self::new(RuntimeValidation::BlockOnFromWorker)
    }

    pub(crate) fn shutdown_wait_from_worker() -> Self {
        Self::new(RuntimeValidation::ShutdownWaitFromWorker)
    }

    pub(crate) fn shutdown_wait_from_blocking_callback() -> Self {
        Self::new(RuntimeValidation::ShutdownWaitFromBlockingCallback)
    }

    pub(crate) fn io_outside_worker() -> Self {
        Self::new(RuntimeValidation::IoOutsideWorker)
    }

    pub(crate) fn runtime_shutting_down() -> Self {
        Self::new(RuntimeValidation::RuntimeShuttingDown)
    }

    pub(crate) fn io_failure(error: Arc<DriverError>) -> Self {
        Self::new(IoFailure(error))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeValidation {
    InvalidWorkerCount,
    InvalidBlockingPoolLimit,
    BlockOnFromWorker,
    ShutdownWaitFromWorker,
    ShutdownWaitFromBlockingCallback,
    IoOutsideWorker,
    RuntimeShuttingDown,
}

impl Display for RuntimeValidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidWorkerCount => "worker count must be greater than zero",
            Self::InvalidBlockingPoolLimit => "blocking pool max_workers must be greater than zero",
            Self::BlockOnFromWorker => "block_on cannot be called from an async Arty worker",
            Self::ShutdownWaitFromWorker => "an async Arty worker cannot wait for runtime shutdown",
            Self::ShutdownWaitFromBlockingCallback => "a runtime blocking callback cannot wait for its own shutdown",
            Self::IoOutsideWorker => "I/O context access requires an active associated worker",
            Self::RuntimeShuttingDown => "the runtime is shutting down",
        })
    }
}

impl StdError for RuntimeValidation {}

#[derive(Debug)]
struct IoFailure(Arc<DriverError>);

impl Display for IoFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.0.as_ref(), f)
    }
}

impl StdError for IoFailure {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.0.as_ref())
    }
}

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

    #[test]
    fn validation_failures_preserve_private_identity_and_display_context() {
        let cases = [
            (
                Error::invalid_worker_count(),
                RuntimeValidation::InvalidWorkerCount,
                "worker count must be greater than zero",
            ),
            (
                Error::invalid_blocking_pool_limit(),
                RuntimeValidation::InvalidBlockingPoolLimit,
                "blocking pool max_workers must be greater than zero",
            ),
            (
                Error::block_on_from_worker(),
                RuntimeValidation::BlockOnFromWorker,
                "block_on cannot be called from an async Arty worker",
            ),
            (
                Error::shutdown_wait_from_worker(),
                RuntimeValidation::ShutdownWaitFromWorker,
                "an async Arty worker cannot wait for runtime shutdown",
            ),
            (
                Error::shutdown_wait_from_blocking_callback(),
                RuntimeValidation::ShutdownWaitFromBlockingCallback,
                "a runtime blocking callback cannot wait for its own shutdown",
            ),
        ];

        for (error, validation, message) in cases {
            assert_eq!(error.to_string(), message);
            assert_eq!(error.source().unwrap().downcast_ref::<RuntimeValidation>(), Some(&validation));
        }
    }

    #[test]
    fn io_failure_preserves_driver_error_as_its_source() {
        let error = Error::io_failure(Arc::new(DriverError::from_message("driver failed")));
        let source = error.source().unwrap();

        assert_eq!(source.to_string(), "driver failed");
        assert_eq!(source.source().unwrap().to_string(), "driver failed");
    }
}
