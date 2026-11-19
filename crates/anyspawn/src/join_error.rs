// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use ohno::OhnoCore;

/// A spawned task failed to return its result.
///
/// [`JoinHandle`](crate::JoinHandle) returns this error when the runtime reports
/// a task panic, cancellation, or shutdown rejection. The original runtime error
/// remains available through [`std::error::Error::source`].
///
/// Task-returned errors are separate. A task returning `Result<T, E>` produces
/// `Result<Result<T, E>, JoinError>` when joined.
///
/// This error does not change the runtime's panic policy. Arty can still shut
/// down after a task panic. With `panic = "abort"`, a panic terminates the process.
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "tokio")]
/// # #[tokio::main]
/// # async fn main() {
/// use anyspawn::Spawner;
///
/// let spawner = Spawner::new_tokio(None);
/// match spawner.spawn(async { 42 }).await {
///     Ok(value) => assert_eq!(value, 42),
///     Err(error) if error.is_cancelled() => eprintln!("Task was cancelled: {error}"),
///     Err(error) => eprintln!("Task failed: {error}"),
/// }
/// # }
/// # #[cfg(not(feature = "tokio"))]
/// # fn main() {}
/// ```
#[derive(ohno::Error)]
#[no_constructors]
#[display("spawned task did not complete")]
pub struct JoinError {
    panicked: bool,
    cancelled: bool,
    ohno_core: OhnoCore,
}

impl JoinError {
    /// Returns `true` if the runtime caught a task panic.
    #[must_use]
    pub fn is_panic(&self) -> bool {
        self.panicked
    }

    /// Returns `true` if the task was cancelled or rejected during shutdown.
    ///
    /// Arty reports these cases as shutdown. Tokio reports them as cancellation.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    #[cfg(any(test, feature = "arty"))]
    pub(crate) fn from_arty(error: arty::task::JoinError) -> Self {
        Self {
            panicked: error.is_panic(),
            cancelled: error.is_shutdown(),
            ohno_core: OhnoCore::from(error),
        }
    }

    #[cfg(any(test, feature = "tokio"))]
    pub(crate) fn from_tokio(error: tokio::task::JoinError) -> Self {
        Self {
            panicked: error.is_panic(),
            cancelled: error.is_cancelled(),
            ohno_core: OhnoCore::from(error),
        }
    }
}
