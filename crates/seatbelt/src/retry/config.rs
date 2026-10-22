// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::retry::Backoff;
use crate::retry::constants::{DEFAULT_BACKOFF, DEFAULT_BASE_DELAY, DEFAULT_RETRY_ATTEMPTS, DEFAULT_USE_JITTER};

/// Configuration for the retry middleware.
///
/// Enable `serde` to load this type from a configuration file.
/// Use [`RetryLayer::config`][crate::retry::RetryLayer::config] to apply it to a retry layer.
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
/// | `handle_unavailable` | `false` |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct RetryConfig {
    /// Controls if the retry middleware runs.
    ///
    /// If this value is `false`, requests go directly to the inner service.
    pub enabled: bool,

    /// Selects the backoff strategy for retry delays.
    pub backoff_type: Backoff,

    /// Sets the base delay for backoff calculations.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub base_delay: Duration,

    /// Sets the maximum delay between retries.
    ///
    /// `None` means that there is no limit.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::optional")
    )]
    pub max_delay: Option<Duration>,

    /// Controls if retry delays include jitter.
    pub use_jitter: bool,

    /// Sets the maximum number of retries after the original call.
    pub max_retry_attempts: u32,

    /// Controls if [`RecoveryInfo::unavailable()`][crate::RecoveryInfo::unavailable] permits a retry.
    ///
    /// If this value is `false`, the middleware returns unavailable output immediately.
    pub handle_unavailable: bool,
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
            handle_unavailable: false,
        }
    }
}
