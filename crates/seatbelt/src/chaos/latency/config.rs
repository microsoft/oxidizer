// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Default latency rate (no latency injection).
const DEFAULT_RATE: f64 = 0.0;

/// Configuration for the [`Latency`][super::Latency] middleware.
///
/// This type can be deserialized from configuration files when the `serde`
/// feature is enabled.
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
    /// Whether the latency middleware is enabled. When `false`, the middleware
    /// is bypassed and requests pass through directly to the inner service.
    pub enabled: bool,

    /// The probability of injecting latency before calling the inner service.
    /// Must be in the range `[0.0, 1.0]` where `0.0` means never inject and
    /// `1.0` means always inject.
    pub rate: f64,

    /// The latency duration to inject. When [`max_latency`][LatencyConfig::max_latency]
    /// is `None`, this is used as a fixed delay. When `max_latency` is `Some`,
    /// this is the lower bound of the random range `[latency, max_latency)`.
    #[cfg_attr(any(feature = "serde", test), serde(with = "duration_millis"))]
    pub latency: Duration,

    /// Optional upper bound for a random latency range. When set, the injected
    /// latency is chosen uniformly at random from `[latency, max_latency)`.
    #[cfg_attr(any(feature = "serde", test), serde(default, with = "option_duration_millis"))]
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

#[cfg(any(feature = "serde", test))]
mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        duration.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

#[cfg(any(feature = "serde", test))]
mod option_duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error> {
        duration.map(|d| d.as_millis()).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Duration>, D::Error> {
        let millis = Option::<u64>::deserialize(deserializer)?;
        Ok(millis.map(Duration::from_millis))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn default_snapshot() {
        let config = LatencyConfig::default();
        insta::assert_json_snapshot!(config);
    }

    #[test]
    fn serde_roundtrip() {
        let config = LatencyConfig {
            enabled: false,
            rate: 0.42,
            latency: Duration::from_millis(100),
            max_latency: Some(Duration::from_millis(500)),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: LatencyConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config, deserialized);
    }

    #[test]
    fn serde_roundtrip_without_max() {
        let config = LatencyConfig {
            enabled: true,
            rate: 0.5,
            latency: Duration::from_millis(200),
            max_latency: None,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: LatencyConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config, deserialized);
    }
}
