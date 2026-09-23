// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

/// Runtime facilities supplied when an I/O provider is created.
///
/// These options are empty in the current contract. Their private representation allows future
/// versions to add optional runtime facilities without changing [`IoContext::provider`][crate::IoContext::provider].
pub struct ProviderOptions {
    _private: (),
}

impl ProviderOptions {
    /// Creates provider options.
    ///
    /// This constructor is intended for runtime implementations and provider tests.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for ProviderOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ProviderOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderOptions").finish_non_exhaustive()
    }
}
