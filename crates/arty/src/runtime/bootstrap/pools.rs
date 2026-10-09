// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::runtime::blocking_worker::BlockingPool;

/// Resolves whether workers share their blocking-task pool.
#[derive(Debug, Clone)]
pub(in crate::runtime) enum BlockingPools {
    Shared(BlockingPool),
    PerWorker(Option<NonZeroUsize>),
}

impl BlockingPools {
    pub(in crate::runtime) const fn per_worker(max_workers: Option<NonZeroUsize>) -> Self {
        Self::PerWorker(max_workers)
    }

    pub(in crate::runtime) fn shared(max_workers: Option<NonZeroUsize>) -> Self {
        Self::Shared(BlockingPool::new_with_mode(max_workers, "shared"))
    }

    pub(super) fn build_worker(&self) -> BlockingPool {
        match self {
            Self::Shared(pool) => pool.clone(),
            Self::PerWorker(max_workers) => BlockingPool::new_with_mode(*max_workers, "per_worker"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn shared_policy_reuses_the_same_blocking_pool() {
        let pools = BlockingPools::shared(NonZeroUsize::new(1));
        assert!(pools.build_worker().shares_pool_with(&pools.build_worker()));
    }

    #[test]
    fn per_worker_policy_creates_independent_blocking_pools() {
        let pools = BlockingPools::per_worker(NonZeroUsize::new(1));
        assert!(!pools.build_worker().shares_pool_with(&pools.build_worker()));
    }
}
