// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Arguments passed to the [`rate_with`][super::LatencyLayer::rate_with] callback.
///
/// This type is `#[non_exhaustive]` so that additional fields can be added in the
/// future without a breaking change.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct LatencyRateArgs {}

/// Arguments passed to the [`latency_with`][super::LatencyLayer::latency_with] callback.
///
/// This type is `#[non_exhaustive]` so that additional fields can be added in the
/// future without a breaking change.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct LatencyDurationArgs {}
