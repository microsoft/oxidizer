// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(test)]

use arty::runtime::hook::OnPanic;
use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};

pub(crate) fn runtime() -> Runtime {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .on(OnPanic::IGNORE)
        .build()
        .unwrap()
}
