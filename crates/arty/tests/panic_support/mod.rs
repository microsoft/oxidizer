// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(test)]

use arty::runtime::Runtime;
use arty::runtime::hook::Panic;
use arty::runtime::policy::{BlockingPoolPolicy, WorkersPolicy};

pub(crate) fn runtime() -> Runtime {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .on(Panic::ignore())
        .build()
        .unwrap()
}
