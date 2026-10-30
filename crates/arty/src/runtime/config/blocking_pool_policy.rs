// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::runtime::error::Error;
use crate::runtime::worker::blocking::Pools;

/// A sharing policy for blocking-task thread pools.
///
/// Pass a policy to
/// [`RuntimeBuilder::blocking_pool`](crate::runtime::RuntimeBuilder::blocking_pool).
/// These pools run [`spawn_blocking`](crate::task::Scheduler::spawn_blocking)
/// callbacks separately from async workers.
///
/// # Choosing between per-worker and shared
///
/// [`per_worker`](Self::per_worker) gives each async worker its own pool. This
/// separates blocking work between workers, but can use more threads as
/// the number of workers grows.
///
/// [`shared`](Self::shared), the default, gives all workers one pool with a
/// common thread limit. Chain [`max`](Self::max) to cap the threads in either
/// the shared pool or each per-worker pool.
///
/// Choose based on your workload. Neither policy is faster in every case.
/// Pool sizing must also account for dependency cycles. A blocking callback
/// that waits for async work which in turn waits for the same saturated pool
/// can deadlock. Shared pools make the dependency runtime-wide. Per-worker
/// pools confine it to one async worker pool. Arty does not track transitive
/// task dependencies to detect such cycles.
///
/// # Examples
///
/// ```
/// use arty::runtime::{BlockingPoolPolicy, Runtime};
///
/// let builder = Runtime::builder().blocking_pool(BlockingPoolPolicy::shared().max(4));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockingPoolPolicy {
    mode: Mode,
    max_workers: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Copy)]
enum Mode {
    PerWorker,
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
    /// let builder = Runtime::builder().blocking_pool(BlockingPoolPolicy::per_worker());
    /// ```
    #[must_use]
    pub const fn per_worker() -> Self {
        Self {
            mode: Mode::PerWorker,
            max_workers: None,
        }
    }

    /// Creates a policy sharing one blocking pool across all async workers.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool(BlockingPoolPolicy::shared().max(4));
    /// ```
    #[must_use]
    pub const fn shared() -> Self {
        Self {
            mode: Mode::Shared,
            max_workers: None,
        }
    }

    /// Sets the maximum number of threads in the shared pool or in each
    /// per-worker pool.
    ///
    /// A zero count is retained in the policy and rejected by
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build).
    #[must_use]
    pub const fn max(mut self, max_workers: usize) -> Self {
        self.max_workers = Some(max_workers);
        self
    }

    /// Resolves this configuration into the concrete pool resources used by
    /// workers at runtime.
    pub(crate) fn into_pools(self) -> Result<Pools, Error> {
        match self.mode {
            Mode::PerWorker => {
                let max_workers = validated_max(self.max_workers)?;
                Ok(Pools::per_worker(max_workers))
            }
            Mode::Shared => {
                let max_workers = validated_max(self.max_workers)?;
                Ok(Pools::shared(max_workers))
            }
        }
    }
}

impl Default for BlockingPoolPolicy {
    fn default() -> Self {
        Self::shared()
    }
}

fn validated_max(max_workers: Option<usize>) -> Result<Option<NonZeroUsize>, Error> {
    max_workers
        .map(|max_workers| NonZeroUsize::new(max_workers).ok_or_else(Error::invalid_blocking_pool_limit))
        .transpose()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn per_worker_into_pools_is_per_worker() {
        assert!(
            matches!(BlockingPoolPolicy::per_worker().into_pools().unwrap(), Pools::PerWorker(None)),
            "expected PerWorker variant"
        );
    }

    #[test]
    fn shared_into_pools_is_shared() {
        assert!(
            matches!(BlockingPoolPolicy::shared().into_pools().unwrap(), Pools::Shared(_)),
            "expected Shared variant"
        );
    }

    #[test]
    fn maximum_applies_to_both_pool_modes() {
        assert!(matches!(
            BlockingPoolPolicy::per_worker().max(3).into_pools().unwrap(),
            Pools::PerWorker(Some(max)) if max.get() == 3
        ));
        assert!(matches!(
            BlockingPoolPolicy::shared().max(4).into_pools().unwrap(),
            Pools::Shared(pool) if pool.max_thread_count() == 4
        ));
    }
}
