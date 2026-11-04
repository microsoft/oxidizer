// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Default latency rate (no latency injection).
const DEFAULT_RATE: f64 = 0.0;

/// Configuration for the [`Latency`][super::Latency] middleware.
///
/// Enable `serde` to load this type from a configuration file.
///
/// # Example
///
/// ```rust
/// use std::time::Duration;
///
/// use seatbelt::chaos::latency::LatencyConfig;
///
/// let config = LatencyConfig::default();
/// assert!(config.enabled);
/// assert_eq!(config.rate, 0.0);
/// assert_eq!(config.latency, Duration::ZERO);
/// assert!(config.max_latency.is_none());
/// ```
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct LatencyConfig {
    /// Controls whether the latency middleware runs.
    ///
    /// If this value is `false`, requests go directly to the inner service.
    pub enabled: bool,

    /// Sets the rate for latency injection.
    ///
    /// The value must be in `[0.0, 1.0]`.
    /// `0.0` never adds latency, and `1.0` always adds latency.
    pub rate: f64,

    /// Sets the latency to add.
    ///
    /// If [`max_latency`][LatencyConfig::max_latency] is `None`, this value is a fixed delay.
    /// Otherwise, this value is the lower bound of `[latency, max_latency)`.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub latency: Duration,

    /// Sets the optional upper bound for random latency.
    ///
    /// If set, the middleware selects a value from `[latency, max_latency)`.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(default, with = "jiff::fmt::serde::unsigned_duration::friendly::compact::optional")
    )]
    pub max_latency: Option<Duration>,
}

impl Default for LatencyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rate: DEFAULT_RATE,
            latency: Duration::ZERO,
            max_latency: None,
        }
    }
}
