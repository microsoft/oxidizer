// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverContext, DriverError, IoContext};

/// Creates and connects one driver type's per-worker instances.
///
/// A runtime keeps one provider per registered driver type. For each worker it clones and
/// relocates the provider, then consumes that clone to create the worker's driver instance.
/// Shared queues, registries, engines, and driver-owned threads remain private provider state.
pub trait DriverProvider: Clone + ThreadAware + 'static {
    /// The context type whose request selects this provider.
    type Context: IoContext<Provider = Self>;

    /// Creates the driver instance serving one async worker.
    ///
    /// This method runs on the thread that will own the returned driver. It must return promptly
    /// and must never wait for another worker's creation or for async workers to make progress.
    /// Consuming the relocated provider clone makes the one-creation-per-worker lifecycle
    /// explicit. The returned boxed trait object is local: it is neither [`Send`] nor [`Sync`]
    /// even when the concrete driver is.
    ///
    /// Select the native strategy here from the client capabilities the supplied context actually
    /// contains, before performing native side effects. Shared, strategy-specific native
    /// initialization may be deferred into the provider's shared state and performed on the first
    /// creation that needs it, provided it completes without waiting for another worker.
    ///
    /// The runtime supplies coherent worker configurations: it does not discover a usable
    /// intersection of client capabilities across differently configured workers on the
    /// provider's behalf. A worker whose configuration cannot support this driver yields an
    /// error, and the runtime rolls back the instances it already created.
    ///
    /// Native routing and notification must be established before this method returns success.
    /// If creation fails, partial registrations must be released or safely retained until native
    /// callbacks can no longer access them.
    ///
    /// # Errors
    ///
    /// Returns unsupported configuration, native initialization, or registration failures.
    /// Environmental failures are reported as errors, not required panics.
    fn create(self, context: DriverContext) -> Result<Box<dyn Driver<Context = Self::Context>>, DriverError>;
}
