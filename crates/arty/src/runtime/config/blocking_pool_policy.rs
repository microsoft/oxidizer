// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::runtime::error::Error;
use crate::runtime::worker::blocking::Pools;

/// Controls whether async workers share the threads that run blocking functions.
///
/// Pass a policy to
/// [`RuntimeBuilder::blocking_pool`](crate::runtime::RuntimeBuilder::blocking_pool).
/// These pools run [`spawn_blocking`](crate::task::Scheduler::spawn_blocking)
/// functions separately from async workers.
///
/// # Choosing between per-worker and shared
///
/// [`per_worker`](Self::per_worker) gives each async worker its own pool. This
/// separates blocking functions between workers, but can use more threads as
/// the number of workers grows.
///
/// [`shared`](Self::shared), the default, gives all workers one pool with a
/// common thread limit. Chain [`max`](Self::max) to cap the threads in either
/// the shared pool or each per-worker pool.
///
/// Choose based on your workload. Neither policy is faster in every case.
/// If all pool threads wait for tasks that need the same pool, none can finish.
/// This is called a deadlock. It can happen when a blocking function waits
/// for an async task that submits another blocking function to the full pool.
/// With a shared pool, this can affect blocking functions from any worker.
/// With separate pools, it affects blocking functions sent to the same worker's pool.
/// Arty does not track chains of waiting tasks to detect these cycles.
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
    /// Each pool uses the runtime's default blocking-thread limit. More async
    /// workers therefore allow more blocking threads in total.
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
    /// This method accepts zero, but
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) rejects it.
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
