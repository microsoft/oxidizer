// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Provides data to the [`fallback`][super::FallbackLayer::fallback] and
/// [`fallback_async`][super::FallbackLayer::fallback_async] actions.
///
/// This type is `#[non_exhaustive]`, so later versions can add fields.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct FallbackActionArgs {}
