// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use many_cpus::ProcessorSet;

use crate::runtime::config::BlockingPoolPolicy;
use crate::runtime::error::Error;

/// A worker-count policy for the async runtime.
///
/// The runtime starts one async worker per selected processor. Pass a
/// policy to
/// [`RuntimeBuilder::workers`](crate::runtime::RuntimeBuilder::workers).
/// The default, [`auto`](Self::auto), lets Arty choose the count. Use
/// [`at_most`](Self::at_most) for an upper bound, [`exactly`](Self::exactly) when
/// fewer workers would be an error, or [`all`](Self::all) to require all available
/// processors.
///
/// Counts are validated when the runtime is built. Both `at_most(0)` and
/// `exactly(0)` produce a construction error.
///
/// # Examples
///
/// ```
/// use arty::runtime::{Runtime, WorkersPolicy};
///
/// let builder = Runtime::builder().workers(WorkersPolicy::at_most(4));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkersPolicy(WorkersPolicyKind);

impl WorkersPolicy {
    /// Creates a policy requiring exactly `count` workers.
    ///
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns
    /// [`Error`] if `count` is zero or fewer processors are available. Creating
    /// the policy does not validate the count. Use [`at_most`](Self::at_most) if
    /// the application can work with fewer workers.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::exactly(4));
    /// ```
    #[must_use]
    pub const fn exactly(count: usize) -> Self {
        Self(WorkersPolicyKind::Exactly(count))
    }

    /// Creates a policy using at most `count` workers.
    ///
    /// Fewer available processors means fewer workers, not a construction error.
    /// A zero count is rejected by
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build), not when
    /// creating this policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::at_most(4));
    /// ```
    #[must_use]
    pub const fn at_most(count: usize) -> Self {
        Self(WorkersPolicyKind::AtMost(count))
    }

    /// Creates a policy using all available processors.
    ///
    /// This explicitly selects all processors rather than relying on
    /// [`auto`](Self::auto)'s default policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::all());
    /// ```
    #[must_use]
    pub const fn all() -> Self {
        Self(WorkersPolicyKind::All)
    }

    /// Creates the default policy, allowing Arty to choose the worker count.
    ///
    /// The selection policy may evolve. It currently uses all available processors.
    /// Use an explicit policy when the worker count matters to the application.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::auto());
    /// ```
    #[must_use]
    pub const fn auto() -> Self {
        Self(WorkersPolicyKind::Auto)
    }

    pub(crate) fn select(&self, available: &ProcessorSet) -> Result<ProcessorSet, Error> {
        let count = match self.0 {
            WorkersPolicyKind::Exactly(0) | WorkersPolicyKind::AtMost(0) => {
                return Err(Error::new("worker count must be greater than zero"));
            }
            WorkersPolicyKind::Exactly(count) if count > available.len() => {
                return Err(Error::insufficient_processors(count, available.len()));
            }
            WorkersPolicyKind::Exactly(count) | WorkersPolicyKind::AtMost(count) => count.min(available.len()),
            WorkersPolicyKind::Auto | WorkersPolicyKind::All => {
                return Ok(available.to_builder().take_all().expect("available processor sets are nonempty"));
            }
        };
        let count = NonZero::new(count).expect("zero counts were rejected above and available processor sets are nonempty");
        Ok(available
            .to_builder()
            .take(count)
            .expect("the requested count fits in the available processor set"))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WorkersPolicyKind {
    #[default]
    Auto,
    Exactly(usize),
    AtMost(usize),
    All,
}

/// Resource settings validated during runtime construction.
#[derive(Debug, PartialEq)]
pub(crate) struct RuntimeConfig {
    pub(crate) workers_policy: WorkersPolicy,
    pub(crate) stack_size: usize,
    pub(crate) blocking_pool_policy: BlockingPoolPolicy,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            workers_policy: WorkersPolicy::default(),
            // Match Rust's Tier-1 thread-stack baseline instead of platform-native defaults.
            // The builder can override it; bootstrap also honors a larger RUST_MIN_STACK.
            stack_size: 2 * 1024 * 1024,
            blocking_pool_policy: BlockingPoolPolicy::default(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    #[cfg(not(miri))]
    use many_cpus::SystemHardware;

    use super::*;

    #[test]
    fn default_runtime_shares_blocking_pool() {
        assert_eq!(RuntimeConfig::default().blocking_pool_policy, BlockingPoolPolicy::shared(None));
    }

    #[cfg(not(miri))]
    #[test]
    fn exact_count_selects_requested_processors() {
        const COUNT: WorkersPolicy = WorkersPolicy::exactly(1);
        let available = SystemHardware::current().processors();
        let selected = COUNT.select(&available).unwrap();
        assert_eq!(selected.len(), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn exact_count_accepts_all_available_processors() {
        let available = SystemHardware::current().processors().take(NonZero::new(1).unwrap()).unwrap();
        let selected = WorkersPolicy::exactly(available.len()).select(&available).unwrap();

        assert_eq!(selected.len(), available.len());
    }

    #[cfg(not(miri))]
    #[test]
    fn unavailable_exact_count_returns_error() {
        let available = SystemHardware::current().processors().take(NonZero::new(1).unwrap()).unwrap();
        WorkersPolicy::exactly(2).select(&available).unwrap_err();
    }

    #[cfg(not(miri))]
    #[test]
    fn maximum_count_clamps_to_available_processors() {
        let available = SystemHardware::current().processors();
        let selected = WorkersPolicy::at_most(usize::MAX).select(&available).unwrap();
        assert_eq!(selected.len(), available.len());
    }

    #[cfg(not(miri))]
    #[test]
    fn maximum_count_caps_processor_selection() {
        const COUNT: WorkersPolicy = WorkersPolicy::at_most(1);
        let available = SystemHardware::current().processors();
        let selected = COUNT.select(&available).unwrap();
        assert_eq!(selected.len(), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn automatic_count_uses_all_available_processors() {
        let available = SystemHardware::current().processors();
        assert_eq!(WorkersPolicy::auto().select(&available).unwrap().len(), available.len(),);
    }

    #[cfg(not(miri))]
    #[test]
    fn all_processors_uses_all_available_processors() {
        const COUNT: WorkersPolicy = WorkersPolicy::all();
        let available = SystemHardware::current().processors();
        assert_eq!(COUNT.select(&available).unwrap().len(), available.len());
    }
}
