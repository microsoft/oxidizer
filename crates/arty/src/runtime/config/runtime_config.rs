// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::runtime::config::{BlockingPoolPolicy, MonitorOptions, PanicPolicy, RuntimeName, WorkersPolicy};

/// Resource settings validated during runtime construction.
#[derive(Debug)]
pub(crate) struct RuntimeConfig {
    pub(crate) name: RuntimeName,
    pub(crate) workers_policy: WorkersPolicy,
    pub(crate) stack_size: NonZeroUsize,
    pub(crate) blocking_pool_policy: BlockingPoolPolicy,
    pub(crate) monitor_options: MonitorOptions,
    pub(crate) panic_policy: PanicPolicy,
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
            panic_policy: PanicPolicy::default(),
        }
    }
}
