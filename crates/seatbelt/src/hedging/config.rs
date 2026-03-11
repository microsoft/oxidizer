// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::hedging::constants::{DEFAULT_HEDGING_DELAY, DEFAULT_MAX_HEDGED_ATTEMPTS};

/// Configuration for the hedging middleware.
///
/// This struct provides a serialization-friendly way to configure the hedging middleware
/// from external sources such as configuration files. Use [`HedgingLayer::config`][crate::hedging::HedgingLayer::config] to apply
/// the configuration to a hedging layer.
///
/// # Defaults
///
/// The default values match the hedging middleware defaults:
///
/// | Field | Default |
/// |-------|---------|
/// | `enabled` | `true` |
/// | `hedging_delay` | 500 milliseconds |
/// | `max_hedged_attempts` | `1` |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HedgingConfig {
    /// Whether the hedging middleware is enabled. When `false`, the middleware
    /// is bypassed and requests pass through directly to the inner service.
    pub enabled: bool,

    /// The delay between launching hedging attempts.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub hedging_delay: Duration,

    /// The maximum number of additional hedged attempts (not counting the original call).
    pub max_hedged_attempts: u8,
}

impl Default for HedgingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hedging_delay: DEFAULT_HEDGING_DELAY,
            max_hedged_attempts: DEFAULT_MAX_HEDGED_ATTEMPTS,
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
        let config = HedgingConfig::default();
        insta::assert_json_snapshot!(config);
    }
}
