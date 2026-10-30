// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects how an Arty runtime responds to a contained task or callback panic.
///
/// Pass a policy to
/// [`RuntimeBuilder::panic`](crate::runtime::RuntimeBuilder::panic).
/// The shutdown policy is the default.
///
/// Arty applies this policy when it catches a panic while it creates or polls
/// an async task, or runs a blocking callback. See the
/// [panic handling guide](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md)
/// for the exact boundaries and limitations.
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
    inner: arty_executor::PanicPolicy,
}

impl PanicPolicy {
    /// Creates a policy that starts coordinated runtime shutdown after a
    /// contained task or callback panic.
    #[must_use]
    pub const fn shutdown() -> Self {
        Self {
            inner: arty_executor::PanicPolicy::Shutdown,
        }
    }

    /// Creates a policy that contains the panic and keeps the runtime running.
    ///
    /// The panicking task or callback still fails through its join handle.
    /// Arty isolates most task panics from runtime internals, but it
    /// cannot repair user state. If user code is not panic-safe, this policy
    /// can leave shared user state corrupted.
    #[must_use]
    pub const fn ignore() -> Self {
        Self {
            inner: arty_executor::PanicPolicy::Ignore,
        }
    }

    pub(crate) fn into_executor_policy(self) -> arty_executor::PanicPolicy {
        self.inner
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
        assert!(matches!(
            PanicPolicy::default().into_executor_policy(),
            arty_executor::PanicPolicy::Shutdown
        ));
    }

    #[test]
    fn ignore_policy_keeps_the_runtime_running() {
        assert!(matches!(
            PanicPolicy::ignore().into_executor_policy(),
            arty_executor::PanicPolicy::Ignore
        ));
    }
}
