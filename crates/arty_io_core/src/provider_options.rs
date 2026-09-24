// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Options for creating a driver provider.
///
/// This type is currently empty. Its private representation allows compatible versions to add
/// optional runtime facilities.
#[derive(Debug)]
#[non_exhaustive]
pub struct ProviderOptions;

#[expect(
    clippy::new_without_default,
    reason = "provider options intentionally require explicit construction"
)]
impl ProviderOptions {
    /// Creates an empty set of provider options.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
