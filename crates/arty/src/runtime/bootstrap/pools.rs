// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::runtime::blocking_worker::WorkerPool;

/// Resolves whether workers share their blocking-task pool.
#[derive(Debug, Clone)]
pub(in crate::runtime) enum WorkerPools {
    Shared(WorkerPool),
    Isolated,
}

impl WorkerPools {
    pub(in crate::runtime) const fn isolated() -> Self {
        Self::Isolated
    }

    pub(in crate::runtime) fn shared(max_workers: Option<usize>) -> Self {
        Self::Shared(WorkerPool::new(max_workers))
    }

    pub(super) fn build_worker(&self) -> WorkerPool {
        match self {
            Self::Shared(pool) => pool.clone(),
            Self::Isolated => WorkerPool::new(None),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn shared_policy_reuses_the_same_blocking_pool() {
        let pools = WorkerPools::shared(Some(1));
        assert!(pools.build_worker().shares_pool_with(&pools.build_worker()));
    }

    #[test]
    fn isolated_policy_creates_independent_blocking_pools() {
        let pools = WorkerPools::isolated();
        assert!(!pools.build_worker().shares_pool_with(&pools.build_worker()));
    }
}
