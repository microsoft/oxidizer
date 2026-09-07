// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::DriverProvider;

/// A consumer-facing I/O handle that identifies the provider used to create it.
///
/// A runtime can initialize the associated driver from the requested context type alone:
///
/// ```text
/// runtime.get_context::<MyContext>()
/// ```
///
/// On the first request, the runtime calls [`provider`](Self::provider), registers that provider,
/// and creates its per-worker driver instances. Later requests reuse the registered driver and do
/// not call `provider` again.
pub trait DriverContext: Clone + ThreadAware + 'static {
    /// The provider that creates the driver behind this context.
    type Provider: DriverProvider<Context = Self>;

    /// Returns the provider used when this context type is first requested.
    ///
    /// The runtime gives this function once-semantics per context type. Implementations should
    /// therefore return an equivalent provider on every call and should not rely on calls after
    /// successful registration.
    #[must_use]
    fn provider() -> Self::Provider;
}
