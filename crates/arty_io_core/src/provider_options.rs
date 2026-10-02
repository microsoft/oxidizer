// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Options supplied by a runtime when creating a driver provider.
///
/// No runtime facilities are currently exposed.
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
