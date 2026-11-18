// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects the runtime's monitoring configuration.
///
/// Pass a policy to
/// [`RuntimeBuilder::monitor`](crate::runtime::RuntimeBuilder::monitor).
/// The enabled policy is the default.
///
/// The `monitor` Cargo feature enables the Seismograph dependencies that
/// monitoring needs. Building an enabled runtime starts a local monitor.
/// Task collection and poll timing run only when requested. This policy does
/// not enable event recording or install an allocator.
///
/// # Examples
///
/// ```no_run
/// use arty::runtime::{MonitorPolicy, Runtime};
///
/// let runtime = Runtime::builder()
///     .monitor(MonitorPolicy::enabled())
///     .build()?;
/// runtime.stop()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct MonitorPolicy {
    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(dead_code, reason = "The policy is only read when monitoring is compiled in.")
    )]
    enabled: bool,
}

impl MonitorPolicy {
    /// Creates a policy with monitoring enabled in the runtime configuration.
    #[must_use]
    pub const fn enabled() -> Self {
        Self { enabled: true }
    }

    /// Creates a policy with monitoring disabled in the runtime configuration.
    #[must_use]
    pub const fn disabled() -> Self {
        Self { enabled: false }
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for MonitorPolicy {
    fn default() -> Self {
        Self::enabled()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
// Monitoring currently stores configuration without a public observable effect.
mod tests {
    use super::*;

    #[test]
    fn default_policy_enables_monitoring() {
        assert!(MonitorPolicy::default().is_enabled());
    }

    #[test]
    fn disabled_policy_disables_monitoring() {
        assert!(!MonitorPolicy::disabled().is_enabled());
    }
}
