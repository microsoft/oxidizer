// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

use crate::breaker::HalfOpenMode;
use crate::breaker::constants::{DEFAULT_BREAK_DURATION, DEFAULT_FAILURE_THRESHOLD, DEFAULT_MIN_THROUGHPUT, DEFAULT_SAMPLING_DURATION};

/// Configuration for the circuit breaker middleware.
///
/// This struct provides a serialization-friendly way to configure the circuit breaker middleware
/// from external sources such as configuration files. Use [`BreakerLayer::config`][crate::breaker::BreakerLayer::config] to apply
/// the configuration to a breaker layer.
///
/// # Defaults
///
/// The default values match the circuit breaker middleware defaults:
///
/// | Field | Default |
/// |-------|---------|
/// | `failure_threshold` | `0.1` (10%) |
/// | `min_throughput` | `100` |
/// | `sampling_duration` | 30 seconds |
/// | `break_duration` | 5 seconds |
/// | `half_open_mode` | `Progressive` (no custom stage duration) |
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct BreakerConfig {
    /// The failure threshold (0.0 to 1.0) at which the circuit opens.
    pub failure_threshold: f32,

    /// The minimum number of executions in the sampling window before the circuit can open.
    pub min_throughput: u32,

    /// The time window for calculating failure rates.
    pub sampling_duration: Duration,

    /// The duration the circuit remains open before transitioning to half-open.
    pub break_duration: Duration,

    /// The behavior of the circuit breaker when transitioning from half-open to closed state.
    pub half_open_mode: HalfOpenMode,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            min_throughput: DEFAULT_MIN_THROUGHPUT,
            sampling_duration: DEFAULT_SAMPLING_DURATION,
            break_duration: DEFAULT_BREAK_DURATION,
            half_open_mode: HalfOpenMode::progressive(None),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot() {
        let config = BreakerConfig::default();
        insta::assert_json_snapshot!(config);
    }
}
