// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod define_fn_wrapper;
use std::sync::Arc;

pub(crate) use define_fn_wrapper::define_fn_wrapper;

#[cfg(any(feature = "metrics", test))]
mod attributes;
#[cfg(any(feature = "metrics", test))]
pub(crate) use attributes::*;

mod telemetry_helper;
pub(crate) use telemetry_helper::TelemetryHelper;

/// Controls whether a middleware is enabled for a given input.
///
/// This enum has three modes:
/// - `Enabled`: The middleware is always active (default).
/// - `Disabled`: The middleware is always bypassed.
/// - `Custom`: The middleware is conditionally active based on a user-provided predicate.
#[derive(Default)]
pub(crate) enum EnableIf<In> {
    /// The middleware is always enabled.
    #[default]
    Enabled,
    /// The middleware is always disabled.
    Disabled,
    /// The middleware is conditionally enabled based on a predicate.
    Custom(Arc<dyn for<'a> Fn(&'a In) -> bool + Send + Sync>),
}

impl<In> EnableIf<In> {
    /// Creates `Enabled` when `true`, `Disabled` when `false`.
    pub fn new(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }

    /// Creates a new `EnableIf` with a custom predicate.
    pub fn custom(predicate: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        Self::Custom(Arc::new(predicate))
    }

    /// Evaluates whether the middleware is enabled for the given input.
    #[cfg_attr(test, mutants::skip)] // causes test timeouts
    pub fn call(&self, input: &In) -> bool {
        match self {
            Self::Enabled => true,
            Self::Disabled => false,
            Self::Custom(predicate) => predicate(input),
        }
    }
}

impl<In> Clone for EnableIf<In> {
    fn clone(&self) -> Self {
        match self {
            Self::Enabled => Self::Enabled,
            Self::Disabled => Self::Disabled,
            Self::Custom(predicate) => Self::Custom(Arc::clone(predicate)),
        }
    }
}

impl<In> std::fmt::Debug for EnableIf<In> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enabled => write!(f, "Enabled"),
            Self::Disabled => write!(f, "Disabled"),
            Self::Custom(_) => write!(f, "Custom"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_if_enabled_debug() {
        let enable_if: EnableIf<String> = EnableIf::default();
        assert_eq!(format!("{enable_if:?}"), "Enabled");
    }

    #[test]
    fn enable_if_disabled_debug() {
        let enable_if: EnableIf<String> = EnableIf::new(false);
        assert_eq!(format!("{enable_if:?}"), "Disabled");
    }

    #[test]
    fn enable_if_custom_debug() {
        let enable_if: EnableIf<String> = EnableIf::custom(|_| true);
        assert_eq!(format!("{enable_if:?}"), "Custom");
    }
}
