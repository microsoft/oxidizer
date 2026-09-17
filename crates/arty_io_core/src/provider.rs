// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{CompletionRequirements, Driver, DriverContext, DriverError, IoContext, LocalDriver};

/// Creates and connects one driver type's per-worker instances.
///
/// A runtime keeps one provider per registered driver type. For each worker it clones and
/// relocates the provider, then consumes that clone to create the worker's driver instance.
/// Shared queues, registries, and driver-owned threads remain private provider state.
pub trait DriverProvider: Clone + ThreadAware + Sized + 'static {
    /// The context type whose request selects this provider.
    type Context: IoContext<Provider = Self>;

    /// The driver type created by this provider.
    type Driver: Driver<Context = Self::Context>;

    /// Returns the client capabilities needed by the strategy this provider selected.
    ///
    /// The default needs no native client services. A provider may use
    /// [`ProviderContext::offers`](crate::ProviderContext::offers) to choose between alternative
    /// strategies before declaring this conjunctive set. The runtime validates these
    /// requirements on each final owning thread before creating or publishing driver instances.
    fn completion_requirements(&self) -> CompletionRequirements {
        CompletionRequirements::new()
    }

    /// Creates the driver instance serving one async worker.
    ///
    /// This method runs on the thread that will own the returned driver. It must return promptly
    /// and must not wait for async workers to make progress. Consuming the relocated provider clone
    /// makes the one-creation-per-worker lifecycle explicit.
    ///
    /// Native routing and notification must be established before this method returns success.
    /// If creation fails, partial registrations must be released or safely retained until native
    /// callbacks can no longer access them. The runtime rolls back previously created instances
    /// rather than publishing a partially registered driver.
    ///
    /// # Errors
    ///
    /// Returns unsupported configuration, native initialization, or registration failures.
    /// Environmental failures are reported as errors, not required panics.
    fn create(self, context: DriverContext) -> Result<LocalDriver<Self::Driver>, DriverError>;
}
