// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies warmed spawn-and-complete operations reuse storage.

#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]

mod support;

use std::alloc::System;

use alloc_tracker::{Allocator, Session};
use arty_executor::{CycleOutcome, Executor, TaskOptions, TaskSet};
use support::DiscardCompletion;

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const WARM_UP_OPERATIONS: usize = 2;
const MEASURED_OPERATIONS: usize = 1_000;
const EXPECTED_ALLOCATIONS_PER_OPERATION: u64 = 0;

fn spawn_complete_once(executor: &mut Executor, tasks: &TaskSet) {
    tasks.add(async {}, TaskOptions::default(), DiscardCompletion);
    let _ = executor.execute_cycle();
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
fn spawn_and_complete_one_reuses_all_storage() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();

    for _ in 0..WARM_UP_OPERATIONS {
        spawn_complete_once(&mut executor, &tasks);
    }
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("spawn_and_complete_one");
    {
        let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
        for _ in 0..MEASURED_OPERATIONS {
            spawn_complete_once(&mut executor, &tasks);
        }
    }

    let (allocations, bytes) = allocation_totals(&session, "spawn_and_complete_one");
    assert_eq!(
        allocations,
        EXPECTED_ALLOCATIONS_PER_OPERATION * MEASURED_OPERATIONS as u64,
        "warmed spawn-and-complete operations must not allocate"
    );
    assert_eq!(
        bytes, 0,
        "warmed task registration, polling, result consumption, and reclamation must reuse existing storage"
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    executor.shutdown().trigger(arty_executor::ShutdownCause::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
}
