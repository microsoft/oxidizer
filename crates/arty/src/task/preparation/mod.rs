// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task preparation and deferred execution wrappers.
//!
//! Submission code owns admission decisions. These helpers construct executable
//! payloads and their result channels without deciding whether work is accepted.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::discard_task_panic_payload;

mod blocking;
mod deferred_task;
mod remote;

pub(crate) use blocking::prepare_blocking;
pub(crate) use deferred_task::DeferredTask;
pub(crate) use remote::{RemoteFutureFactory, prepare_remote};

fn discard_panicking_drop<T>(value: T) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(value))) {
        discard_task_panic_payload(panic);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests;
