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
/// ```
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
    enabled: bool,
    name: Option<String>,
    #[cfg(feature = "monitor")]
    io: Option<seismograph_protocol::io::MonitorIo>,
}

impl MonitorPolicy {
    /// Creates a policy with monitoring enabled in the runtime configuration.
    #[must_use]
    pub const fn enabled() -> Self {
        Self {
            enabled: true,
            name: None,
            #[cfg(feature = "monitor")]
            io: None,
        }
    }

    /// Creates a policy with monitoring disabled in the runtime configuration.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            name: None,
            #[cfg(feature = "monitor")]
            io: None,
        }
    }

    /// Sets the runtime name shown by the Seismograph CLI.
    ///
    /// The default name is `arty`. Each runtime has a separate local endpoint.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Selects isolated monitor I/O for tests.
    ///
    /// This does not change the process-wide recorder or runtime source.
    #[cfg(all(feature = "monitor", any(test, feature = "test-util")))]
    #[must_use]
    pub fn io(mut self, io: seismograph_protocol::io::MonitorIo) -> Self {
        self.io = Some(io);
        self
    }

    pub(crate) const fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[cfg(feature = "monitor")]
    pub(crate) fn runtime_name(&self) -> &str {
        self.name.as_deref().unwrap_or("arty")
    }

    #[cfg(feature = "monitor")]
    pub(crate) fn monitor_io(&self) -> Option<seismograph_protocol::io::MonitorIo> {
        #[cfg(all(miri, any(test, feature = "test-util")))]
        if self.io.is_none() {
            return Some(seismograph_protocol::io::MonitorIo::new_mocked().0);
        }
        self.io.clone()
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
