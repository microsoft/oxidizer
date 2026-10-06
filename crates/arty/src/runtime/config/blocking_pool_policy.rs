// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::bootstrap::pools::BlockingPools;
use crate::runtime::error::Error;

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
    /// async worker count. A zero count is retained in the policy and rejected
    /// by [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build).
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
        Self {
            mode: Mode::Shared,
            max_workers: max_workers.into(),
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
    pub(in crate::runtime) fn into_pools(self) -> Result<BlockingPools, Error> {
        match self.mode {
            Mode::Isolated => Ok(BlockingPools::isolated()),
            Mode::Shared if self.max_workers == Some(0) => Err(Error::new("blocking pool max_workers must be greater than zero")),
            Mode::Shared => Ok(BlockingPools::shared(self.max_workers)),
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
    fn isolated_into_pools_is_isolated() {
        assert!(
            matches!(BlockingPoolPolicy::isolated().into_pools().unwrap(), BlockingPools::Isolated),
            "expected Isolated variant"
        );
    }

    #[test]
    fn shared_into_pools_is_shared() {
        assert!(
            matches!(BlockingPoolPolicy::shared(None).into_pools().unwrap(), BlockingPools::Shared(_)),
            "expected Shared variant"
        );
    }
}
