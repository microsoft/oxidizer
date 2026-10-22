// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Default injection rate (no injection).
const DEFAULT_RATE: f64 = 0.0;

/// Configuration for the [`Injection`][super::Injection] middleware.
///
/// Enable `serde` to load this type from a configuration file.
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
    /// Controls if the injection middleware runs.
    ///
    /// If this value is `false`, requests go directly to the inner service.
    pub enabled: bool,

    /// Sets the rate for output injection.
    ///
    /// The value must be in `[0.0, 1.0]`.
    /// `0.0` never injects output, and `1.0` always injects output.
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
