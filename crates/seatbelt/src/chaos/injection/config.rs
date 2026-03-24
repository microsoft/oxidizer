// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Default injection rate (no injection).
const DEFAULT_RATE: f64 = 0.0;

/// Configuration for the [`Injection`][super::Injection] middleware.
///
/// This type can be deserialized from configuration files when the `serde`
/// feature is enabled.
///
/// # Example
///
/// ```rust
/// use seatbelt::chaos::injection::InjectionConfig;
///
/// let config = InjectionConfig::default();
/// assert!(config.enabled);
/// assert_eq!(config.rate, 0.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct InjectionConfig {
    /// Whether the injection middleware is enabled. When `false`, the middleware
    /// is bypassed and requests pass through directly to the inner service.
    pub enabled: bool,

    /// The probability of injecting the configured output instead of calling the
    /// inner service. Must be in the range `[0.0, 1.0]` where `0.0` means never
    /// inject and `1.0` means always inject.
    pub rate: f64,
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rate: DEFAULT_RATE,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::float_cmp, reason = "exact comparison is intentional for default constant")]
    fn default_values() {
        let config = InjectionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.rate, 0.0);
    }

    #[test]
    fn serde_roundtrip() {
        let config = InjectionConfig {
            enabled: false,
            rate: 0.42,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: InjectionConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config, deserialized);
    }
}
