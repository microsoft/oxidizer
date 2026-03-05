// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Default timeout duration: 30 seconds.
///
/// This default provides a reasonable timeout for most service-to-service
/// communication scenarios.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for the timeout middleware.
///
/// This struct provides a serialization-friendly way to configure the timeout middleware
/// from external sources such as configuration files. Use [`TimeoutLayer::config`][crate::timeout::TimeoutLayer::config] to apply
/// the configuration to a timeout layer.
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
    /// Whether the timeout middleware is enabled. When `false`, the middleware
    /// is bypassed and requests pass through directly to the inner service.
    pub enabled: bool,

    /// The timeout duration for operations.
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn default_snapshot() {
        let config = TimeoutConfig::default();
        insta::assert_json_snapshot!(config);
    }
}
