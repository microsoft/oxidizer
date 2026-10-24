// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies warmed spawn-and-complete operations reuse storage.

#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::alloc::System;
use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker};

use alloc_tracker::{Allocator, Session};
use arty_executor::remote::Pool as RemotePool;
use arty_executor::{CycleOutcome, Executor, JoinError, TaskOptions, TaskSet};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const WARM_UP_OPERATIONS: usize = 2;
const MEASURED_OPERATIONS: usize = 1_000;
#[cfg(debug_assertions)]
const EXPECTED_ALLOCATIONS_PER_OPERATION: u64 = 1;
#[cfg(not(debug_assertions))]
const EXPECTED_ALLOCATIONS_PER_OPERATION: u64 = 0;
#[cfg(debug_assertions)]
const EXPECTED_REMOTE_ALLOCATIONS_PER_OPERATION: u64 = 1;
#[cfg(not(debug_assertions))]
const EXPECTED_REMOTE_ALLOCATIONS_PER_OPERATION: u64 = 0;

fn spawn_complete_once(executor: &mut Executor, tasks: &TaskSet) -> Poll<Result<(), JoinError>> {
    let mut handle = pin!(tasks.add(async {}, TaskOptions::default()));
    let _ = executor.execute_cycle();
    handle.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

fn spawn_remote_complete_once(executor: &mut Executor, tasks: &TaskSet, result_pool: &RemotePool) -> Poll<Result<(), JoinError>> {
    let (result_tx, mut handle) = result_pool.channel();
    tasks.add_with_remote_result(async {}, TaskOptions::default(), result_tx);
    let _ = executor.execute_cycle();
    Pin::new(&mut handle).poll(&mut Context::from_waker(Waker::noop()))
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
fn spawn_and_complete_one_only_allocates_debug_bookkeeping() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();

    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(spawn_complete_once(&mut executor, &tasks), Poll::Ready(Ok(())));
    }
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("spawn_and_complete_one");
    {
        let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
        for _ in 0..MEASURED_OPERATIONS {
            assert_eq!(spawn_complete_once(&mut executor, &tasks), Poll::Ready(Ok(())));
        }
    }

    let (allocations, bytes) = allocation_totals(&session, "spawn_and_complete_one");
    assert_eq!(
        allocations,
        EXPECTED_ALLOCATIONS_PER_OPERATION * MEASURED_OPERATIONS as u64,
        "warmed spawn-and-complete operations must allocate only one debug bookkeeping object and nothing in release builds"
    );
    #[cfg(not(debug_assertions))]
    assert_eq!(
        bytes, 0,
        "warmed task registration, polling, result consumption, and reclamation must reuse existing storage"
    );
    #[cfg(debug_assertions)]
    assert_ne!(bytes, 0, "debug bookkeeping must be visible to the allocation tracker");

    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);
    executor.begin_shutdown();
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}

#[test]
fn remote_spawn_reuses_cross_thread_result_channel_storage() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let result_pool = RemotePool::new();

    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(spawn_remote_complete_once(&mut executor, &tasks, &result_pool), Poll::Ready(Ok(())));
    }
    assert_eq!(executor.execute_cycle(), CycleOutcome::Suspend);

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("spawn_remote_complete_one");
    {
        let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
        for _ in 0..MEASURED_OPERATIONS {
            assert_eq!(spawn_remote_complete_once(&mut executor, &tasks, &result_pool), Poll::Ready(Ok(())));
        }
    }

    let (allocations, bytes) = allocation_totals(&session, "spawn_remote_complete_one");
    assert_eq!(
        allocations,
        EXPECTED_REMOTE_ALLOCATIONS_PER_OPERATION * MEASURED_OPERATIONS as u64,
        "warmed remote registration must allocate only debug bookkeeping and nothing in release builds"
    );
    #[cfg(not(debug_assertions))]
    assert_eq!(bytes, 0, "warmed remote result channels must reuse pooled storage");
    #[cfg(debug_assertions)]
    assert_ne!(bytes, 0, "debug bookkeeping must be visible to the allocation tracker");

    executor.begin_shutdown();
    assert_eq!(executor.execute_cycle(), CycleOutcome::Shutdown);
}
