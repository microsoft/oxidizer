// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects how an Arty runtime will respond to a contained task panic.
///
/// Pass a policy to
/// [`RuntimeBuilder::panic`](crate::runtime::RuntimeBuilder::panic).
/// The shutdown policy is the default.
///
/// This release stores the policy in the runtime configuration. It does not
/// yet change panic behavior. A follow-up change will enforce the policy at
/// the documented panic boundaries.
///
/// # Examples
///
/// Select the ignore policy:
///
/// ```
/// use arty::runtime::{PanicPolicy, Runtime};
///
/// let builder = Runtime::builder().panic(PanicPolicy::ignore());
/// ```
#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct PanicPolicy {
    shutdown: bool,
}

impl PanicPolicy {
    /// Creates a policy that selects coordinated runtime shutdown after a
    /// contained panic.
    #[must_use]
    pub const fn shutdown() -> Self {
        Self { shutdown: true }
    }

    /// Creates a policy that selects continued runtime operation after a
    /// contained panic.
    #[must_use]
    pub const fn ignore() -> Self {
        Self { shutdown: false }
    }

    pub(crate) const fn starts_shutdown(&self) -> bool {
        self.shutdown
    }
}

impl Default for PanicPolicy {
    fn default() -> Self {
        Self::shutdown()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn default_policy_selects_shutdown() {
        assert!(PanicPolicy::default().starts_shutdown());
    }

    #[test]
    fn ignore_policy_does_not_select_shutdown() {
        assert!(!PanicPolicy::ignore().starts_shutdown());
    }
}
