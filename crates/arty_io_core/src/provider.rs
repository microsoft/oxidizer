// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{Driver, DriverContext, IoContext};

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

    /// Creates the driver instance serving one async worker.
    ///
    /// This method runs on the thread that will own the returned driver. It must return promptly
    /// and must not wait for async workers to make progress. Consuming the relocated provider clone
    /// makes the one-creation-per-worker lifecycle explicit.
    ///
    /// # Panics
    ///
    /// Panics when this worker's driver instance cannot be initialized. Driver registration is
    /// runtime-fundamental: after one worker fails to initialize, the runtime cannot continue in a
    /// coherent partially registered state.
    fn create(self, context: DriverContext) -> Self::Driver;
}
