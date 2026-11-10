// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::runtime::worker::blocking::Pool;

/// Resolves whether workers share their blocking-task pool.
#[derive(Debug, Clone)]
pub(crate) enum Pools {
    Shared(Pool),
    PerWorker { name: Box<str>, max_workers: Option<NonZeroUsize> },
}

impl Pools {
    pub(crate) fn per_worker(name: &str, max_workers: Option<NonZeroUsize>) -> Self {
        Self::PerWorker {
            name: name.into(),
            max_workers,
        }
    }

    pub(crate) fn shared(name: &str, max_workers: Option<NonZeroUsize>) -> Self {
        Self::Shared(Pool::new(name, max_workers))
    }

    pub(crate) fn pool_for_worker(&self) -> Pool {
        match self {
            Self::Shared(pool) => pool.clone(),
            Self::PerWorker { name, max_workers } => Pool::new(name, *max_workers),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn shared_policy_reuses_the_same_blocking_pool() {
        let pools = Pools::shared("arty-blocking", NonZeroUsize::new(1));
        assert!(pools.pool_for_worker().shares_pool_with(&pools.pool_for_worker()));
    }

    #[test]
    fn per_worker_policy_creates_independent_blocking_pools() {
        let pools = Pools::per_worker("arty-blocking", NonZeroUsize::new(1));
        assert!(!pools.pool_for_worker().shares_pool_with(&pools.pool_for_worker()));
    }
}
