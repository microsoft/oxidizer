// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;
use std::sync::Mutex;

use crate::runtime::config::{BlockingPoolPolicy, MonitorOptions, RuntimeName, WorkersPolicy};
use crate::runtime::hook::Hooks;

/// Resource settings validated during runtime construction.
#[derive(Debug)]
pub(crate) struct RuntimeConfig {
    pub(crate) name: RuntimeName,
    pub(crate) workers_policy: WorkersPolicy,
    pub(crate) stack_size: NonZeroUsize,
    pub(crate) blocking_pool_policy: BlockingPoolPolicy,
    pub(crate) monitor_options: MonitorOptions,
    pub(crate) hooks: Mutex<Hooks>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            name: RuntimeName::default(),
            workers_policy: WorkersPolicy::default(),
            // Match Rust's Tier-1 thread-stack baseline instead of platform-native defaults.
            // The builder can override it; bootstrap also honors a larger RUST_MIN_STACK.
            stack_size: NonZeroUsize::new(2 * 1024 * 1024).expect("the default stack size is nonzero"),
            blocking_pool_policy: BlockingPoolPolicy::default(),
            monitor_options: MonitorOptions::default(),
            hooks: Mutex::new(Hooks::default()),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn default_runtime_shares_blocking_pool() {
        assert_eq!(RuntimeConfig::default().blocking_pool_policy, BlockingPoolPolicy::shared());
    }
}
