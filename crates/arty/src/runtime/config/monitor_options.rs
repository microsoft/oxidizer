// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::config::MonitorPolicy;

/// Worker-owned monitoring configuration derived from the runtime policy.
#[derive(Clone, Debug)]
pub(crate) struct MonitorOptions {
    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(dead_code, reason = "The policy is only read when monitoring is compiled in.")
    )]
    policy: MonitorPolicy,
    #[cfg(any(test, feature = "monitor"))]
    runtime: Option<seismograph_runtime::RuntimeHandle>,
}

impl MonitorOptions {
    /// Creates the copy owned by one async worker.
    pub(crate) fn for_worker(&self) -> Self {
        self.clone()
    }

    /// Returns the policy retained by this worker.
    #[cfg(any(test, feature = "monitor"))]
    pub(crate) const fn policy(&self) -> &MonitorPolicy {
        &self.policy
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn set_runtime(&mut self, runtime: Option<seismograph_runtime::RuntimeHandle>) {
        self.runtime = runtime;
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn runtime(&self) -> Option<&seismograph_runtime::RuntimeHandle> {
        self.runtime.as_ref()
    }
}

impl Default for MonitorOptions {
    fn default() -> Self {
        MonitorPolicy::default().into()
    }
}

impl From<MonitorPolicy> for MonitorOptions {
    fn from(policy: MonitorPolicy) -> Self {
        Self {
            policy,
            #[cfg(any(test, feature = "monitor"))]
            runtime: None,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
// Monitoring currently stores configuration without a public observable effect.
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(MonitorOptions: Clone, Send, Sync);
    assert_not_impl_any!(MonitorOptions: Copy);

    #[test]
    fn default_options_disable_monitoring_in_tests() {
        assert!(!MonitorOptions::default().policy().is_enabled());
    }

    #[test]
    fn worker_options_preserve_the_runtime_policy() {
        let options = MonitorOptions::from(MonitorPolicy::disabled());

        assert!(!options.for_worker().policy().is_enabled());
    }
}
