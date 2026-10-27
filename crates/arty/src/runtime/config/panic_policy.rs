// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects how an Arty runtime responds to a contained task panic.
///
/// Pass a policy to
/// [`RuntimeBuilder::panic`](crate::runtime::RuntimeBuilder::panic).
/// The shutdown policy is the default.
///
/// Arty applies this policy when it catches a panic while it creates or polls
/// an async task, or while it runs a blocking callback. See
/// [the panic guide](crate::documentation::panics) for the exact boundaries and
/// limitations.
///
/// # Examples
///
/// Keep the runtime available after a contained task panic:
///
/// ```
/// use arty::runtime::{PanicPolicy, Runtime};
///
/// let runtime = Runtime::builder().panic(PanicPolicy::ignore()).build()?;
/// runtime.stop()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[repr(transparent)]
#[derive(Debug, Clone)]
pub struct PanicPolicy {
    shutdown: bool,
}

impl PanicPolicy {
    /// Creates a policy that starts coordinated runtime shutdown after a
    /// contained panic.
    #[must_use]
    pub const fn shutdown() -> Self {
        Self { shutdown: true }
    }

    /// Creates a policy that contains the panic and keeps the runtime running.
    ///
    /// The panicking task or blocking callback still fails through its join
    /// handle.
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
    fn default_policy_starts_shutdown() {
        assert!(PanicPolicy::default().starts_shutdown());
    }

    #[test]
    fn ignore_policy_keeps_the_runtime_running() {
        assert!(!PanicPolicy::ignore().starts_shutdown());
    }
}
