// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::retry::backoff::Backoff;
use crate::retry::constants::{DEFAULT_BACKOFF, DEFAULT_BASE_DELAY, DEFAULT_RETRY_ATTEMPTS, DEFAULT_USE_JITTER};

/// Configuration for the retry middleware.
///
/// This struct provides a serialization-friendly way to configure the retry middleware
/// from external sources such as configuration files. Use [`RetryLayer::config`][crate::retry::RetryLayer::config] to apply
/// the configuration to a retry layer.
///
/// # Defaults
///
/// The default values match the retry middleware defaults:
///
/// | Field | Default |
/// |-------|---------|
/// | `enabled` | `true` |
/// | `backoff_type` | [`Backoff::Exponential`] |
/// | `base_delay` | 10 milliseconds |
/// | `max_delay` | `None` |
/// | `use_jitter` | `true` |
/// | `max_retry_attempts` | `3` |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct RetryConfig {
    /// Whether the retry middleware is enabled. When `false`, the middleware
    /// is bypassed and requests pass through directly to the inner service.
    pub enabled: bool,

    /// The backoff strategy to use for retry delays.
    pub backoff_type: Backoff,

    /// The base delay used for backoff calculations.
    pub base_delay: Duration,

    /// The maximum allowed delay between retries. `None` means no limit.
    pub max_delay: Option<Duration>,

    /// Whether to add jitter to delay calculations.
    pub use_jitter: bool,

    /// The maximum number of retry attempts (not counting the original call).
    pub max_retry_attempts: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backoff_type: DEFAULT_BACKOFF,
            base_delay: DEFAULT_BASE_DELAY,
            max_delay: None,
            use_jitter: DEFAULT_USE_JITTER,
            max_retry_attempts: DEFAULT_RETRY_ATTEMPTS,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn default_snapshot() {
        let config = RetryConfig::default();
        insta::assert_json_snapshot!(config);
    }
}
