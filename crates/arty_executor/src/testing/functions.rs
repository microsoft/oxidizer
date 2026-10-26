// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::thread;

use scopeguard::{Always, ScopeGuard};

use crate::{CycleOutcome, Executor, ShutdownCause};

pub(crate) fn new_test_task_handle() -> crate::TaskHandle {
    crate::allocate_test_task()
}

/// Creates an executor with unit-test cleanup that drives explicit shutdown.
#[must_use]
#[cfg_attr(test, mutants::skip)] // This is test scaffolding, mutation here is unhelpful.
pub(crate) fn new_guarded_executor(owner_waker: Waker) -> ScopeGuard<Executor, fn(Executor), Always> {
    scopeguard::guard(Executor::builder().owner_waker(owner_waker).build(), |mut executor: Executor| {
        executor.begin_shutdown(ShutdownCause::default());

        for _ in 0..16 {
            if executor.execute_cycle() == CycleOutcome::Shutdown {
                return;
            }
            thread::yield_now();
        }

        panic!("executor did not finish shutdown within the bounded cleanup cycles");
    })
}
