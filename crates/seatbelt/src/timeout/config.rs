// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// The default timeout is 30 seconds.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for the timeout middleware.
///
/// Enable `serde` to load this type from a configuration file.
/// Use [`TimeoutLayer::config`][crate::timeout::TimeoutLayer::config] to apply it to a timeout layer.
///
/// # Defaults
///
/// | Field | Default |
/// |-------|---------|
/// | `enabled` | `true` |
/// | `timeout` | 30 seconds |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct TimeoutConfig {
    /// Controls if the timeout middleware runs.
    ///
    /// If this value is `false`, requests go directly to the inner service.
    pub enabled: bool,

    /// Sets the time limit for an operation.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub timeout: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}
