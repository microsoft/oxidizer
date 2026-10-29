// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;
use std::fmt::{self, Display};

use arty_executor::{ShutdownCause, ShutdownError};
use arty_io_core::DriverError;
use performables::arc::Arc;

use crate::runtime::shutdown::Shutdown;

/// An error constructing or operating a runtime.
///
/// You may receive this error when building a runtime with an invalid worker
/// count or blocking-pool limit, waiting from an async worker, or stopping a
/// runtime whose worker or I/O driver failed.
///
/// When [`RuntimeScheduler::block_on`](crate::task::RuntimeScheduler::block_on)
/// fails because a task panicked or was cancelled, its source is a
/// [`JoinError`](crate::task::JoinError).
///
/// Format the error with [`Display`] and inspect [`StdError::source`] for
/// diagnostics. Do not depend on specific messages or source types to classify
/// construction failures. Task joins report `JoinError` directly.
#[derive(Debug)]
pub struct Error {
    source: Box<dyn StdError + Send + Sync>,
}

impl Error {
    pub(crate) fn new(source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self { source: source.into() }
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
        Self::new(IoOutsideWorkerError::new())
    }

    pub(crate) fn runtime_shutting_down() -> Self {
        Self::new(RuntimeShuttingDownError::new())
    }

    pub(crate) fn shutdown_callback_registration() -> Self {
        Self::new(ShutdownCallbackRegistrationError::new())
    }

    pub(crate) fn io_failure(error: Arc<DriverError>) -> Self {
        Self::new(IoFailure::caused_by(SharedDriverError(error)))
    }

    pub(crate) fn shutdown_failure(error: ShutdownError) -> Self {
        Self::new(ShutdownFailure::caused_by(error))
    }

    pub(crate) fn from_shutdown(shutdown: &Shutdown) -> Option<Self> {
        shutdown
            .cause()
            .map(|cause| cause.into_error().map_or_else(Self::runtime_shutting_down, Self::shutdown_failure))
    }

    pub(crate) fn worker_panicked() -> Self {
        Self::new(WorkerPanicked::new())
    }

    #[cfg(any(test, feature = "macros"))]
    pub(crate) fn into_source(self) -> Box<dyn StdError + Send + Sync> {
        self.source
    }

    pub(crate) fn into_shutdown_cause(self) -> ShutdownCause {
        ShutdownError::new(self).into()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeValidation {
    InvalidWorkerCount,
    InvalidBlockingPoolLimit,
    BlockOnFromWorker,
    ShutdownWaitFromWorker,
    ShutdownWaitFromBlockingCallback,
}

impl Display for RuntimeValidation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidWorkerCount => "worker count must be greater than zero",
            Self::InvalidBlockingPoolLimit => "blocking pool max_workers must be greater than zero",
            Self::BlockOnFromWorker => "block_on cannot be called from an async Arty worker",
            Self::ShutdownWaitFromWorker => "an async Arty worker cannot wait for runtime shutdown",
            Self::ShutdownWaitFromBlockingCallback => "a runtime blocking callback cannot wait for its own shutdown",
        })
    }
}

impl StdError for RuntimeValidation {}

#[ohno::error]
#[display("I/O context access cannot synchronously cross between Arty workers")]
struct IoOutsideWorkerError;

#[ohno::error]
#[display("the runtime is shutting down")]
struct RuntimeShuttingDownError;

#[ohno::error]
#[display("runtime shutdown callbacks already completed")]
struct ShutdownCallbackRegistrationError;

#[ohno::error]
struct IoFailure;

#[ohno::error]
struct ShutdownFailure;

#[ohno::error]
#[display("a runtime worker panicked")]
struct WorkerPanicked;

#[derive(Debug)]
struct SharedDriverError(Arc<DriverError>);

impl Display for SharedDriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.0.as_ref(), f)
    }
}

impl StdError for SharedDriverError {
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

        assert!(source.downcast_ref::<IoFailure>().is_some());
        assert_eq!(source.to_string(), "driver failed");
        let shared = source.source().unwrap();
        assert!(shared.downcast_ref::<SharedDriverError>().is_some());
        assert_eq!(shared.to_string(), "driver failed");
        assert!(shared.source().unwrap().downcast_ref::<DriverError>().is_some());
    }

    #[test]
    fn shutdown_failure_preserves_the_shared_shutdown_source() {
        let shutdown = ShutdownError::new(io::Error::other("runtime subsystem failed"));
        let expected = shutdown.clone();
        let error = Error::shutdown_failure(shutdown);
        let source = error.source().unwrap();

        assert!(source.downcast_ref::<ShutdownFailure>().is_some());
        let shutdown = source.source().unwrap().downcast_ref::<ShutdownError>().unwrap();
        let shutdown_source = shutdown.inner().downcast_ref::<io::Error>().unwrap();
        let expected_source = expected.inner().downcast_ref::<io::Error>().unwrap();
        assert!(std::ptr::eq(shutdown_source, expected_source));
        assert_eq!(error.to_string(), "runtime subsystem failed");
    }

    #[test]
    fn io_validation_failures_preserve_private_leaf_identity_and_display_context() {
        let outside_worker = Error::io_outside_worker();
        let shutting_down = Error::runtime_shutting_down();

        assert_eq!(
            outside_worker.to_string(),
            "I/O context access cannot synchronously cross between Arty workers"
        );
        assert!(outside_worker.source().unwrap().downcast_ref::<IoOutsideWorkerError>().is_some());
        assert_eq!(shutting_down.to_string(), "the runtime is shutting down");
        assert!(shutting_down.source().unwrap().downcast_ref::<RuntimeShuttingDownError>().is_some());
    }
}
