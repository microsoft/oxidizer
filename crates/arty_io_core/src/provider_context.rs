// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

/// Runtime facilities supplied when an I/O provider is created.
///
/// This context is empty in the current contract. Its private representation allows future
/// versions to add optional runtime facilities without changing [`IoContext::provider`][crate::IoContext::provider].
pub struct ProviderContext {
    _private: (),
}

impl ProviderContext {
    /// Creates a provider context.
    ///
    /// This constructor is intended for runtime implementations and provider tests.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for ProviderContext {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ProviderContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderContext").finish_non_exhaustive()
    }
}
