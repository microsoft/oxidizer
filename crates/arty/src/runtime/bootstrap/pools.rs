// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::blocking_worker::BlockingPool;

/// Resolves whether workers share their blocking-task pool.
#[derive(Debug, Clone)]
pub(in crate::runtime) enum BlockingPools {
    Shared(BlockingPool),
    Isolated,
}

impl BlockingPools {
    pub(in crate::runtime) const fn isolated() -> Self {
        Self::Isolated
    }

    pub(in crate::runtime) fn shared(max_workers: Option<usize>) -> Self {
        Self::Shared(BlockingPool::new(max_workers))
    }

    pub(super) fn build_worker(&self) -> BlockingPool {
        match self {
            Self::Shared(pool) => pool.clone(),
            Self::Isolated => BlockingPool::new(None),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn shared_policy_reuses_the_same_blocking_pool() {
        let pools = BlockingPools::shared(Some(1));
        assert!(pools.build_worker().shares_pool_with(&pools.build_worker()));
    }

    #[test]
    fn isolated_policy_creates_independent_blocking_pools() {
        let pools = BlockingPools::isolated();
        assert!(!pools.build_worker().shares_pool_with(&pools.build_worker()));
    }
}
