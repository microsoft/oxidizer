// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies warmed spawn-and-complete operations add no allocations beyond debug-only waker diagnostics.

#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::alloc::System;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use alloc_tracker::{Allocator, Session};
use arty_executor::{CycleOutcome, Executor, TaskSet};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const WARM_UP_OPERATIONS: usize = 2;
const MEASURED_OPERATIONS: usize = 1_000;
#[cfg(debug_assertions)]
const EXPECTED_ALLOCATIONS_PER_OPERATION: u64 = 3;
#[cfg(not(debug_assertions))]
const EXPECTED_ALLOCATIONS_PER_OPERATION: u64 = 0;

fn spawn_complete_once(executor: &Executor, tasks: &TaskSet) -> Poll<()> {
    let mut handle = pin!(tasks.add(async {}));
    let _ = executor.execute_cycle();
    handle.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

fn allocation_totals(session: &Session, operation_name: &str) -> (u64, u64) {
    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| {
            (name == operation_name).then(|| (operation.total_allocations_count(), operation.total_bytes_allocated()))
        })
        .unwrap()
}

#[test]
fn spawn_and_complete_one_only_allocates_debug_diagnostics() {
    // SAFETY: The test drives shutdown to `CycleOutcome::Shutdown` before dropping the executor.
    let executor = unsafe { Executor::builder().build() };
    let tasks = executor.tasks();

    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(spawn_complete_once(&executor, &tasks), Poll::Ready(()));
    }
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("spawn_and_complete_one");
    {
        let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
        for _ in 0..MEASURED_OPERATIONS {
            assert_eq!(spawn_complete_once(&executor, &tasks), Poll::Ready(()));
        }
    }

    let (allocations, bytes) = allocation_totals(&session, "spawn_and_complete_one");
    assert_eq!(
        allocations,
        EXPECTED_ALLOCATIONS_PER_OPERATION * MEASURED_OPERATIONS as u64,
        "warmed spawn-and-complete operations must allocate only the three per-task debug diagnostic objects, and nothing in release builds"
    );
    #[cfg(not(debug_assertions))]
    assert_eq!(
        bytes, 0,
        "warmed release-mode task registration, polling, result consumption, and reclamation must reuse existing storage"
    );
    #[cfg(debug_assertions)]
    assert_ne!(bytes, 0, "debug diagnostic objects must be visible to the allocation tracker");

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    executor.begin_shutdown();
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}
