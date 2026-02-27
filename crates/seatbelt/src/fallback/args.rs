// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Arguments passed to the [`before_fallback`][super::FallbackLayer::before_fallback] callback.
///
/// This type is `#[non_exhaustive]` so that additional fields can be added in the
/// future without a breaking change.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct BeforeFallbackArgs {}

/// Arguments passed to the [`after_fallback`][super::FallbackLayer::after_fallback] callback.
///
/// This type is `#[non_exhaustive]` so that additional fields can be added in the
/// future without a breaking change.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct AfterFallbackArgs {}
