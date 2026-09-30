// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::bootstrap::pools::BlockingPools;

/// Controls the thread pools used for blocking tasks.
///
/// The **isolated** policy (the default) gives each async worker its own
/// blocking-task pool.
///
/// The **shared** policy uses one runtime-wide blocking-task pool.
///
/// # Choosing between isolated and shared
///
/// Isolated mode separates blocking-task contention between async workers.
/// The number of blocking-task pools, and their
/// potential thread and stack overhead, scales with the number of async
/// workers.
///
/// Shared mode keeps one pool-wide thread limit regardless of the number of
/// async workers, but combines their blocking tasks in that pool.
///
/// Choose using measured contention and thread-memory costs for the workload.
/// Neither policy guarantees higher throughput for every workload.
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
    /// Uses per-async-worker blocking-task pools (the default).
    ///
    /// Blocking-task thread resources scale with the number of async workers.
    #[must_use]
    pub const fn isolated() -> Self {
        Self {
            mode: Mode::Isolated,
            max_workers: None,
        }
    }

    /// Uses one runtime-wide blocking-task pool shared by all async workers.
    ///
    /// See [the type-level
    /// docs](Self#choosing-between-isolated-and-shared) for the resource and
    /// contention trade-offs.
    ///
    /// `max_workers` sets the upper bound on the number of threads in the pool.
    /// Pass `None` to use the runtime's default limit.
    ///
    /// # Panics
    ///
    /// Panics if `max_workers` is `Some(0)`.
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
