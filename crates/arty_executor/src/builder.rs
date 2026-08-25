// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

use crate::{Executor, ExecutorCore, ShutdownTimeoutBehavior};

/// Builds an instance of [`Executor`].
#[derive(Debug)]
#[must_use]
pub struct ExecutorBuilder {
    shutdown_timeout: Duration,
    shutdown_timeout_behavior: ShutdownTimeoutBehavior,
    owner_waker: Waker,
}

const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

impl ExecutorBuilder {
    /// Creates a new [`ExecutorBuilder`] with default settings.
    pub fn new() -> Self {
        Self {
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            shutdown_timeout_behavior: ShutdownTimeoutBehavior::TerminateProcess,
            owner_waker: Waker::noop().clone(),
        }
    }

    /// The waker that will wake up the executor's owner.
    ///
    /// The `owner_waker` is used by the executor to wake up its owner when more work has
    /// arrived for the executor and calling [`execute_cycle()`][Executor::execute_cycle]
    /// is desirable.
    ///
    /// The default is not to wake the owner even if there is more work the executor could do.
    pub fn owner_waker(mut self, owner_waker: Waker) -> Self {
        self.owner_waker = owner_waker;
        self
    }

    /// How long the executor will wait for cleanup before declaring a shutdown timeout.
    ///
    /// A value of `Duration::ZERO` means shutdown is required to complete immediately,
    /// with the first call to [`execute_cycle()`][Executor::execute_cycle] after
    /// [`begin_shutdown()`][Executor::begin_shutdown].
    ///
    /// The default value is unspecified and may change in future versions.
    ///
    /// # Panics
    ///
    /// Shutdown will panic when it begins if the timeout cannot be represented as a future
    /// [`Instant`][std::time::Instant].
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// What happens when the executor declares a shutdown timeout.
    #[cfg(test)]
    pub(crate) fn shutdown_timeout_behavior(mut self, behavior: ShutdownTimeoutBehavior) -> Self {
        self.shutdown_timeout_behavior = behavior;
        self
    }

    /// Builds the executor with the configured settings.
    ///
    /// # Safety
    ///
    /// The returned object must not be dropped until a call to
    /// [`execute_cycle()`][1] returns [`CycleOutcome::Shutdown`][2].
    ///
    /// [1]: Executor::execute_cycle
    /// [2]: crate::CycleOutcome::Shutdown
    #[must_use]
    pub unsafe fn build(self) -> Executor {
        // SAFETY: Forwarding safety guarantees from caller.
        let core = unsafe { ExecutorCore::new(self.owner_waker, self.shutdown_timeout, self.shutdown_timeout_behavior) };

        Executor::new(core)
    }
}

impl Default for ExecutorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_builder() {
        _ = ExecutorBuilder::default();
    }
}
