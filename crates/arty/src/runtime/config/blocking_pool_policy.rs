// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroU32;

use crate::runtime::error::Error;

const MAX_BLOCKING_WORKERS: u32 = 10_000;
const DEFAULT_SHARED_WORKERS: NonZeroU32 =
    NonZeroU32::new(MAX_BLOCKING_WORKERS).expect("the default shared blocking worker limit is nonzero");

/// A locality and worker-count policy for the runtime's blocking-task pool.
///
/// Pass a policy to
/// [`RuntimeBuilder::blocking_pool`](crate::runtime::RuntimeBuilder::blocking_pool).
/// These pools run [`spawn_blocking`](crate::task::Scheduler::spawn_blocking)
/// callbacks separately from async workers.
///
/// # Choosing locality
///
/// [`per_worker`](Self::per_worker) maps an explicit limit to a
/// processor-local worker group.
///
/// [`shared`](Self::shared), the default, uses one system-wide worker group
/// capped at 10,000 workers unless [`max`](Self::max) selects a lower limit.
/// All modes use one runtime-owned pool.
///
/// Pool sizing must account for dependency cycles. A blocking callback that
/// waits for async work which then waits for the same saturated worker group
/// can deadlock. Arty does not track transitive task dependencies.
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
    /// Creates a processor-local blocking-worker policy.
    ///
    /// Without [`max`](Self::max), the runtime's default worker policy is used.
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

    /// Creates a system-wide shared blocking-worker policy.
    ///
    /// Without [`max`](Self::max), the shared worker group is capped at 10,000
    /// workers. An explicit limit is shared by all submitters.
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

    /// Sets the fixed worker count for the selected locality.
    ///
    /// The supported range is `1..=10_000`. Values outside that range are
    /// retained in the policy and rejected by
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build).
    #[must_use]
    pub const fn max(mut self, max_workers: usize) -> Self {
        self.max_workers = Some(max_workers);
        self
    }

    pub(crate) const fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::PerWorker => "per_worker",
            Mode::Shared => "shared",
        }
    }

    /// Resolves this policy into the `vicinal::WorkerPolicy` for the runtime's single
    /// blocking pool. `None` keeps the default per-processor policy from Vicinal.
    pub(in crate::runtime) fn worker_policy(&self) -> Result<Option<vicinal::WorkerPolicy>, Error> {
        let max_workers = validated_max(self.max_workers)?;

        match self.mode {
            // `per_worker()` used to mean "one dedicated pool per
            // async worker, growing independently". vicinal's default (no explicit
            // WorkerPolicy) schedules each task near its *submitting processor* instead of
            // its arty worker index, which is a finer-grained analogue but not identical.
            Mode::PerWorker => Ok(max_workers.map(vicinal::WorkerPolicy::per_processor)),
            Mode::Shared => Ok(Some(vicinal::WorkerPolicy::per_system(
                max_workers.unwrap_or(DEFAULT_SHARED_WORKERS),
            ))),
        }
    }
}

impl Default for BlockingPoolPolicy {
    fn default() -> Self {
        Self::shared()
    }
}

fn validated_max(max_workers: Option<usize>) -> Result<Option<NonZeroU32>, Error> {
    max_workers
        .map(|max_workers| {
            let max_workers = u32::try_from(max_workers).map_err(|_| Error::blocking_pool_limit_too_large())?;
            let max_workers = NonZeroU32::new(max_workers).ok_or_else(Error::invalid_blocking_pool_limit)?;
            if max_workers.get() > MAX_BLOCKING_WORKERS {
                return Err(Error::blocking_pool_limit_too_large());
            }
            Ok(max_workers)
        })
        .transpose()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn per_worker_resolves_to_default_policy() {
        assert!(
            BlockingPoolPolicy::per_worker().worker_policy().unwrap().is_none(),
            "expected the default (per-processor) vicinal policy"
        );
    }

    #[test]
    fn per_worker_with_max_resolves_to_explicit_policy() {
        assert!(BlockingPoolPolicy::per_worker().max(4).worker_policy().unwrap().is_some());
    }

    #[test]
    fn shared_without_max_resolves_to_system_policy() {
        assert!(
            BlockingPoolPolicy::shared().worker_policy().unwrap().is_some(),
            "expected the default shared system-wide policy"
        );
    }

    #[test]
    fn shared_with_max_resolves_to_system_policy() {
        assert!(BlockingPoolPolicy::shared().max(4).worker_policy().unwrap().is_some());
    }

    #[test]
    fn maximum_supported_limit_resolves() {
        assert!(
            BlockingPoolPolicy::shared()
                .max(MAX_BLOCKING_WORKERS as usize)
                .worker_policy()
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn limit_above_supported_maximum_is_rejected() {
        let error = BlockingPoolPolicy::shared()
            .max(MAX_BLOCKING_WORKERS as usize + 1)
            .worker_policy()
            .unwrap_err();
        assert_eq!(error.to_string(), "blocking pool max_workers must not exceed 10000");
    }
}
