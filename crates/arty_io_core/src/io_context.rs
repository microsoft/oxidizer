// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{DriverProvider, ProviderOptions};

/// A consumer-facing I/O handle associated with a driver provider.
///
/// A runtime uses the concrete context type as the registration key:
///
/// ```text
/// runtime.get_context::<MyContext>()
/// ```
///
/// On the first request, the runtime calls [`provider`](Self::provider) and creates the associated
/// driver on every active worker. Later requests reuse that registration.
pub trait IoContext: Clone + ThreadAware + 'static {
    /// The provider used to create this context's drivers.
    type Provider: DriverProvider<Context = Self>;

    /// Returns the provider for this context type.
    ///
    /// Runtime facilities available during provider creation are supplied in `options`.
    ///
    /// The runtime calls this method at most once for each registered context type. Implementations
    /// should return an equivalent provider for every call.
    #[must_use]
    fn provider(options: ProviderOptions) -> Self::Provider;
}
