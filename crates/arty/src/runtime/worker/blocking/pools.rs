// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::runtime::config::RuntimeName;
use crate::runtime::worker::blocking::Pool;

/// Resolves whether workers share their blocking-task pool.
#[derive(Debug, Clone)]
pub(crate) enum Pools {
    Shared(Pool),
    PerWorker {
        name: RuntimeName,
        max_workers: Option<NonZeroUsize>,
    },
}

impl Pools {
    pub(crate) fn per_worker(name: RuntimeName, max_workers: Option<NonZeroUsize>) -> Self {
        Self::PerWorker { name, max_workers }
    }

    pub(crate) fn shared(name: RuntimeName, max_workers: Option<NonZeroUsize>) -> Self {
        Self::Shared(Pool::new(name, None, max_workers))
    }

    pub(crate) fn pool_for_worker(&self, worker_index: usize) -> Pool {
        match self {
            Self::Shared(pool) => pool.clone(),
            Self::PerWorker { name, max_workers } => Pool::new(name.clone(), Some(worker_index), *max_workers),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
// Pool allocation and shared ownership identity are not exposed by public schedulers.
mod tests {
    use super::*;

    #[test]
    fn shared_policy_reuses_the_same_blocking_pool() {
        let pools = Pools::shared(RuntimeName::default(), NonZeroUsize::new(1));
        assert!(pools.pool_for_worker(0).shares_pool_with(&pools.pool_for_worker(1)));
    }

    #[test]
    fn per_worker_policy_creates_independent_blocking_pools() {
        let pools = Pools::per_worker(RuntimeName::default(), NonZeroUsize::new(1));
        assert!(!pools.pool_for_worker(0).shares_pool_with(&pools.pool_for_worker(1)));
    }
}
