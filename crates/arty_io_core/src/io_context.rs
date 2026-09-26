// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{DriverProvider, ProviderOptions};

/// A consumer-facing I/O handle associated with a driver provider.
///
/// The runtime registers each concrete context type once. On its first request, it creates a
/// driver/context pair on every active worker and completes each driver's zero-wait
/// initialization cycle before publishing the contexts. Later requests reuse the registration.
///
/// See the [single-worker runtime example] for registration and peer discovery with drivers
/// that perform no I/O.
///
/// [single-worker runtime example]: https://github.com/microsoft/oxidizer/tree/main/crates/arty_io_core/examples/single_thread_runtime
pub trait IoContext: Clone + ThreadAware + 'static {
    /// The provider used to create this context and its drivers.
    type Provider: DriverProvider<Context = Self>;

    /// Returns the provider for this context type.
    ///
    /// The runtime calls this method at most once per registered context type, passing
    /// `options`. Repeated calls should return equivalent providers.
    #[must_use]
    fn provider(options: ProviderOptions) -> Self::Provider;
}
