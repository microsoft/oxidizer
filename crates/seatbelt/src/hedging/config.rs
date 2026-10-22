// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::hedging::constants::{DEFAULT_HEDGING_DELAY, DEFAULT_MAX_HEDGED_ATTEMPTS};

/// Configuration for the hedging middleware.
///
/// Enable `serde` to load this type from a configuration file.
/// Use [`HedgingLayer::config`][crate::hedging::HedgingLayer::config] to apply it to a hedging layer.
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
/// | `handle_unavailable` | `false` |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HedgingConfig {
    /// Controls whether the hedging middleware runs.
    ///
    /// If this value is `false`, requests go directly to the inner service.
    pub enabled: bool,

    /// Sets the delay between hedged attempts.
    #[cfg_attr(
        any(feature = "serde", test),
        serde(with = "jiff::fmt::serde::unsigned_duration::friendly::compact::required")
    )]
    pub hedging_delay: Duration,

    /// Sets the maximum number of hedged attempts after the original call.
    pub max_hedged_attempts: u8,

    /// Controls whether [`RecoveryInfo::unavailable()`][crate::RecoveryInfo::unavailable] permits more hedged attempts.
    ///
    /// If this value is `false`, the middleware returns unavailable output immediately.
    pub handle_unavailable: bool,
}

impl Default for HedgingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hedging_delay: DEFAULT_HEDGING_DELAY,
            max_hedged_attempts: DEFAULT_MAX_HEDGED_ATTEMPTS,
            handle_unavailable: false,
        }
    }
}
