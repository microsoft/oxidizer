// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects how an Arty runtime responds to a panic that it catches in a user task.
///
/// Pass a policy to
/// [`RuntimeBuilder::panic`](crate::runtime::RuntimeBuilder::panic).
/// The shutdown policy is the default.
///
/// Arty applies this policy when it catches a panic while it creates or polls
/// an async task, or runs a blocking function. See the
/// [panic handling guide](https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md)
/// for the operations this covers and the limits on recovery.
///
/// # Examples
///
/// Keep the runtime running after Arty catches a task panic:
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
    /// Creates a policy that stops the runtime after Arty catches a panic in a user task.
    #[must_use]
    pub const fn shutdown() -> Self {
        Self {
            inner: arty_executor::PanicPolicy::Shutdown,
        }
    }

    /// Creates a policy that keeps the runtime running after a caught task panic.
    ///
    /// The task's or blocking function's [`JoinHandle`](crate::task::JoinHandle)
    /// still returns an error when awaited.
    /// Arty cannot repair application data. If a panic leaves that data in an
    /// invalid state, this policy can keep the application running with corrupted shared data.
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
