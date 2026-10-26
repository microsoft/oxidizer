// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::config::MonitorPolicy;

/// Worker-owned monitoring configuration derived from the runtime policy.
#[derive(Clone, Debug)]
pub(crate) struct MonitorOptions {
    policy: MonitorPolicy,
}

impl MonitorOptions {
    /// Creates the copy owned by one async worker.
    pub(crate) fn for_worker(&self) -> Self {
        self.clone()
    }

    /// Returns the policy retained by this worker.
    pub(crate) const fn policy(&self) -> &MonitorPolicy {
        &self.policy
    }
}

impl Default for MonitorOptions {
    fn default() -> Self {
        MonitorPolicy::default().into()
    }
}

impl From<MonitorPolicy> for MonitorOptions {
    fn from(policy: MonitorPolicy) -> Self {
        Self { policy }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(MonitorOptions: Clone, Send, Sync);
    assert_not_impl_any!(MonitorOptions: Copy);

    #[test]
    fn default_options_enable_monitoring() {
        assert!(MonitorOptions::default().policy().is_enabled());
    }

    #[test]
    fn worker_options_preserve_the_runtime_policy() {
        let options = MonitorOptions::from(MonitorPolicy::enabled());

        assert!(options.for_worker().policy().is_enabled());
    }
}
