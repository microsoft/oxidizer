// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::bootstrap::pools::BlockingPools;

/// A sharing policy for blocking-task thread pools.
///
/// Pass a policy to
/// [`RuntimeBuilder::blocking_pool_policy`](crate::runtime::RuntimeBuilder::blocking_pool_policy).
/// These pools run [`spawn_blocking`](crate::task::TaskScheduler::spawn_blocking)
/// callbacks separately from asynchronous workers.
///
/// # Choosing between isolated and shared
///
/// [`isolated`](Self::isolated), the default, gives each asynchronous worker its
/// own pool. It separates blocking-task contention between workers, but the
/// number of pools and their thread and stack costs grow with the worker count.
///
/// [`shared`](Self::shared) gives all workers one pool with a common thread
/// limit. It bounds that pool's threads independently of the asynchronous worker
/// count, but all blocking tasks compete for those threads.
///
/// Choose using the workload's contention and thread-memory costs. Neither
/// policy is faster for every workload.
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
    /// Creates the default policy with one pool per asynchronous worker.
    ///
    /// Each pool uses the runtime's default blocking-thread limit. The total
    /// blocking-thread resources therefore grow with the asynchronous worker count.
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

    /// Creates a policy sharing one blocking pool across all asynchronous workers.
    ///
    /// `max_workers` limits the pool's threads. Pass a positive count, or `None`
    /// to use the runtime's default limit. This limit is separate from the
    /// asynchronous worker count.
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
        Self::isolated()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn isolated_is_default() {
        assert_eq!(BlockingPoolPolicy::default(), BlockingPoolPolicy::isolated());
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
