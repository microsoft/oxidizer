// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::bootstrap::pools::BlockingPools;

/// A sharing policy for blocking-task thread pools.
///
/// Pass a policy to
/// [`RuntimeBuilder::blocking_pool_policy`](crate::runtime::RuntimeBuilder::blocking_pool_policy).
/// These pools run [`spawn_blocking`](crate::task::Scheduler::spawn_blocking)
/// callbacks separately from async workers.
///
/// # Choosing between isolated and shared
///
/// [`isolated`](Self::isolated) gives each async worker its own pool. This
/// separates blocking work between workers, but can use more threads as
/// the number of workers grows.
///
/// [`shared`](Self::shared), the default, gives all workers one pool with a
/// common thread limit. Use `shared(n)` to cap that pool's threads.
///
/// Choose based on your workload; neither policy is faster in every case.
///
/// # Examples
///
/// ```
/// use arty::runtime::{BlockingPoolPolicy, Runtime};
///
/// let builder = Runtime::builder().blocking_pool_policy(BlockingPoolPolicy::shared(4));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockingPoolPolicy {
    mode: Mode,
    max_workers: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Copy)]
enum Mode {
    Isolated,
    Shared,
}

impl BlockingPoolPolicy {
    /// Creates a policy with one pool per async worker.
    ///
    /// Each pool uses the runtime's default blocking-thread limit. The total
    /// blocking-thread resources therefore grow with the async worker count.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool_policy(BlockingPoolPolicy::isolated());
    /// ```
    #[must_use]
    pub const fn isolated() -> Self {
        Self {
            mode: Mode::Isolated,
            max_workers: None,
        }
    }

    /// Creates a policy sharing one blocking pool across all async workers.
    ///
    /// `max_workers` limits the pool's threads. Pass a positive count, or `None`
    /// to use the runtime's default limit. This limit is separate from the
    /// async worker count.
    ///
    /// # Panics
    ///
    /// Panics if `max_workers` is zero, including `Some(0)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool_policy(BlockingPoolPolicy::shared(4));
    /// ```
    #[must_use]
    pub fn shared(max_workers: impl Into<Option<usize>>) -> Self {
        let max_workers = max_workers.into();

        assert_ne!(max_workers, Some(0), "max_workers must be non-zero");

        Self {
            mode: Mode::Shared,
            max_workers,
        }
    }

    /// Telemetry label identifying the pool mode (`isolated` / `shared`).
    pub(in crate::runtime) const fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::Isolated => "isolated",
            Mode::Shared => "shared",
        }
    }

    /// Resolves this configuration into the concrete pool resources used by
    /// workers at runtime.
    pub(in crate::runtime) fn into_pools(self) -> BlockingPools {
        match self.mode {
            Mode::Isolated => BlockingPools::isolated(),
            Mode::Shared => BlockingPools::shared(self.max_workers),
        }
    }
}

impl Default for BlockingPoolPolicy {
    fn default() -> Self {
        Self::shared(None)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn shared_is_default() {
        assert_eq!(BlockingPoolPolicy::default(), BlockingPoolPolicy::shared(None));
    }

    #[test]
    fn isolated_into_pools_is_isolated() {
        assert!(
            matches!(BlockingPoolPolicy::isolated().into_pools(), BlockingPools::Isolated),
            "expected Isolated variant"
        );
    }

    #[test]
    fn shared_into_pools_is_shared() {
        assert!(
            matches!(BlockingPoolPolicy::shared(None).into_pools(), BlockingPools::Shared(_)),
            "expected Shared variant"
        );
    }

    #[test]
    #[should_panic(expected = "non-zero")]
    fn shared_zero_panics() {
        let _ = BlockingPoolPolicy::shared(0);
    }
}
