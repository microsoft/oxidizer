// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Provides data to the [`rate_with`][super::LatencyLayer::rate_with] callback.
///
/// This type is `#[non_exhaustive]`, so later versions can add fields.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct LatencyRateArgs {}

/// Provides data to the [`latency_with`][super::LatencyLayer::latency_with] callback.
///
/// This type is `#[non_exhaustive]`, so later versions can add fields.
#[derive(Debug)]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "non_exhaustive requires braces for forward-compatibility"
)]
pub struct LatencyDurationArgs {}
