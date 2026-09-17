// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware_core::ThreadAware;

use crate::{DriverError, DriverProvider};

/// A consumer-facing I/O handle that identifies the provider used to create it.
///
/// A runtime can initialize the associated driver from the requested context type alone:
///
/// ```text
/// runtime.get_context::<MyContext>()
/// ```
///
/// On the first request, the runtime calls [`provider`](Self::provider), registers that provider,
/// and creates its driver instance on every active worker. The request completes only after all
/// workers have initialized the driver. Later requests reuse the registered driver and do not call
/// `provider` again.
pub trait IoContext: Clone + ThreadAware + 'static {
    /// The provider that creates the driver behind this context.
    type Provider: DriverProvider<Context = Self>;

    /// Returns the provider used when this context type is first requested.
    ///
    /// This call establishes shared provider state that every worker instance may use. It carries
    /// no native client capabilities: the completion strategy is selected from the clients that
    /// each owning thread's [`DriverContext`](crate::DriverContext) actually supplies, so
    /// strategy-specific shared native initialization is deferred to the first
    /// [`DriverProvider::create`] that needs it.
    ///
    /// The runtime gives this function once-semantics per context type. Implementations should
    /// therefore return an equivalent provider on every call and should not rely on calls after
    /// successful registration.
    ///
    /// # Errors
    ///
    /// Returns unsupported-environment or provider initialization failures. A failed attempt does
    /// not constitute successful registration and may be retried under an explicit policy.
    fn provider() -> Result<Self::Provider, DriverError>;
}
