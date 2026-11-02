// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects the runtime's monitoring configuration.
///
/// Pass a policy to
/// [`RuntimeBuilder::monitor`](crate::runtime::RuntimeBuilder::monitor).
/// The enabled policy is the default.
///
/// The `monitor` Cargo feature enables the Seismograph dependencies that
/// monitoring needs. This choice does not start a monitor, add task measurements,
/// or change which tasks run.
///
/// # Examples
///
/// ```
/// use arty::runtime::{MonitorPolicy, Runtime};
///
/// let runtime = Runtime::builder()
///     .monitor(MonitorPolicy::enabled())
///     .build()?;
/// runtime.stop()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct MonitorPolicy {
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

    #[cfg(test)]
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
