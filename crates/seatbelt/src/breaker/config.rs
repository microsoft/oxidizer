// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::breaker::constants::{DEFAULT_BREAK_DURATION, DEFAULT_FAILURE_THRESHOLD, DEFAULT_MIN_THROUGHPUT, DEFAULT_SAMPLING_DURATION};
use crate::breaker::{AbandonedPolicy, HalfOpenMode};

/// Configuration for the circuit breaker middleware.
///
/// Enable `serde` to load this type from a configuration file.
/// Use [`BreakerLayer::config`][crate::breaker::BreakerLayer::config] to apply it to a breaker layer.
///
/// # Defaults
///
/// The default values match the circuit breaker middleware defaults:
///
/// | Field | Default |
/// |-------|---------|
/// | `enabled` | `true` |
/// | `failure_threshold` | `0.1` (10%) |
/// | `min_throughput` | `100` |
/// | `sampling_duration` | 30 seconds |
/// | `break_duration` | 5 seconds |
/// | `half_open_mode` | `Progressive` (no custom stage duration) |
/// | `abandoned_policy` | `rate_threshold(1.0)` (counts abandoned only when every execution was abandoned) |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct BreakerConfig {
    /// Controls if the circuit breaker runs.
    ///
    /// If this value is `false`, requests go directly to the inner service.
    pub enabled: bool,

    /// Sets the failure rate that opens the circuit.
    pub failure_threshold: f32,

    /// Sets the minimum number of executions before the circuit can open.
    pub min_throughput: u32,

    /// Sets the time window for the failure rate.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub sampling_duration: Duration,

    /// Sets how long the circuit stays open before it becomes half-open.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub break_duration: Duration,

    /// Selects how a half-open circuit tests service health.
    #[cfg_attr(any(feature = "serde", test), serde(default))]
    pub half_open_mode: HalfOpenMode,

    /// Controls how abandoned executions affect circuit health.
    #[cfg_attr(any(feature = "serde", test), serde(default))]
    pub abandoned_policy: AbandonedPolicy,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            min_throughput: DEFAULT_MIN_THROUGHPUT,
            sampling_duration: DEFAULT_SAMPLING_DURATION,
            break_duration: DEFAULT_BREAK_DURATION,
            half_open_mode: HalfOpenMode::progressive(None),
            abandoned_policy: AbandonedPolicy::default(),
        }
    }
}
