// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::thread;

use scopeguard::{Always, ScopeGuard};

use crate::{CycleOutcome, Executor};

/// Creates an executor with unit-test cleanup that drives explicit shutdown.
#[must_use]
#[cfg_attr(test, mutants::skip)] // This is test scaffolding, mutation here is unhelpful.
pub(crate) fn new_guarded_executor(owner_waker: Waker) -> ScopeGuard<Executor, fn(Executor), Always> {
    scopeguard::guard(Executor::builder().owner_waker(owner_waker).build(), |executor: Executor| {
        executor.begin_shutdown();

        while executor.execute_cycle() != CycleOutcome::Shutdown {
            thread::yield_now();
        }
    })
}
