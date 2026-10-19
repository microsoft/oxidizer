// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::thread;

use scopeguard::{Always, ScopeGuard};

use crate::{CycleOutcome, Executor};

pub(crate) fn new_test_task() -> impl crate::TypeErasedTask {
    let events = events_once::LocalEventPool::<()>::new();
    let (sender, _receiver) = events.rent();
    crate::Task::new(std::future::pending(), sender)
}

pub(crate) fn new_test_task_handle() -> crate::TaskHandle {
    let pool = plurality::Pool::new();
    crate::TaskHandle::new(pool.alloc_arc_pin(new_test_task()))
}

/// Creates an executor with unit-test cleanup that drives explicit shutdown.
#[must_use]
#[cfg_attr(test, mutants::skip)] // This is test scaffolding, mutation here is unhelpful.
pub(crate) fn new_guarded_executor(owner_waker: Waker) -> ScopeGuard<Executor, fn(Executor), Always> {
    scopeguard::guard(Executor::builder().owner_waker(owner_waker).build(), |mut executor: Executor| {
        executor.begin_shutdown();

        while executor.execute_cycle() != CycleOutcome::Shutdown {
            thread::yield_now();
        }
    })
}
