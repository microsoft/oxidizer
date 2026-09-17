// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverContext, DriverError, IoContext, LocalDriver};

/// Creates and connects one driver type's per-worker instances.
///
/// A runtime keeps one provider per registered driver type. For each worker it clones and
/// relocates the provider, then consumes that clone to create the worker's driver instance and
/// the consumer context paired with it. Shared queues, registries, engines, and driver-owned
/// threads remain private provider state.
pub trait DriverProvider: Clone + ThreadAware + 'static {
    /// The context type whose request selects this provider.
    type Context: IoContext<Provider = Self>;

    /// The concrete driver installed on each owning worker.
    type Driver: Driver;

    /// Creates the driver instance serving one async worker, with its consumer context.
    ///
    /// This method runs on the thread that will own the returned driver. It must return promptly
    /// and must never wait for another worker's creation or for async workers to make progress.
    /// Consuming the relocated provider clone makes the one-creation-per-worker lifecycle
    /// explicit. The returned inline owner is neither [`Send`] nor [`Sync`] even when the
    /// concrete driver is. It introduces no heap allocation or dynamic dispatch.
    ///
    /// # Pairing obligation
    ///
    /// The returned context must be the consumer handle of the returned driver instance. This is
    /// a value-level obligation that the signature does not check: returning the two values
    /// together proves neither that they share state nor, since [`Driver`] names no context type,
    /// that the driver belongs to this context family at all. An implementation that creates its
    /// shared state once and derives both values from it satisfies the obligation; one that
    /// returns a context built from another instance silently publishes a handle to the wrong
    /// driver. Provider tests should service or shut down one instance and observe that only its
    /// own context and operations are affected.
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
    fn create(self, context: DriverContext) -> Result<(Self::Context, LocalDriver<Self::Driver>), DriverError>;
}
