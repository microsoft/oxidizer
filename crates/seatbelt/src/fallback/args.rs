// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Arguments passed to the [`fallback`][super::FallbackLayer::fallback] and
/// [`fallback_async`][super::FallbackLayer::fallback_async] actions.
///
/// This type is `#[non_exhaustive]` so that additional fields can be added in the
/// future without a breaking change.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct FallbackActionArgs {}
