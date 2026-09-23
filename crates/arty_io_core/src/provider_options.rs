// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

/// Options for creating a driver provider.
///
/// This type is currently empty. Its private representation allows compatible versions to add
/// optional runtime facilities.
pub struct ProviderOptions {
    _private: (),
}

impl ProviderOptions {
    /// Creates an empty set of provider options.
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
