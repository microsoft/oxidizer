// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies warmed spawn-and-complete operations reuse storage.

#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::alloc::System;
use std::cell::Cell;
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use alloc_tracker::{Allocator, Session};
use arty_executor::{
    CycleOutcome, Executor, ShutdownCause, TaskCompletion, TaskPanicPayload, TaskPoller, TaskSet, discard_task_panic_payload,
};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

const WARM_UP_OPERATIONS: usize = 2;
const MEASURED_OPERATIONS: usize = 1_000;
const EXPECTED_ALLOCATIONS_PER_OPERATION: u64 = 0;

#[derive(Debug)]
struct CountCompletion(Rc<Cell<usize>>);

impl TaskCompletion<()> for CountCompletion {
    fn completed(self, _result: ()) {
        self.0.set(self.0.get().wrapping_add(1));
    }

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

fn spawn_complete_once(executor: &mut Executor, tasks: &TaskSet, completed: &Rc<Cell<usize>>) {
    tasks.task(async {}).register(CountCompletion(Rc::clone(completed)));
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
    let completed = Rc::new(Cell::new(0));

    for _ in 0..WARM_UP_OPERATIONS {
        spawn_complete_once(&mut executor, &tasks, &completed);
    }
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    completed.set(0);

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("spawn_and_complete_one");
    {
        let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
        for _ in 0..MEASURED_OPERATIONS {
            spawn_complete_once(&mut executor, &tasks, &completed);
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

    assert_eq!(completed.get(), MEASURED_OPERATIONS);
    assert!(!executor.has_tasks());
    executor.shutdown().trigger(arty_executor::ShutdownCause::default());
    assert!(matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)));
}

#[derive(Debug)]
struct OneBytePollHook(u8);

impl<F: Future> TaskPoller<F> for OneBytePollHook {
    fn poll(&mut self, mut future: Pin<&mut F>, cx: &mut Context<'_>) -> Result<Poll<F::Output>, TaskPanicPayload> {
        black_box(self.0);
        catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx)))
    }
}

#[test]
fn concrete_poll_hooks_use_no_allocation() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let completed = Rc::new(Cell::new(0));
    for value in 0..WARM_UP_OPERATIONS {
        tasks
            .task(async {})
            .poll_with(OneBytePollHook(value as u8))
            .register(CountCompletion(Rc::clone(&completed)));
        let _ = executor.execute_cycle();
    }
    completed.set(0);

    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("concrete_poll_hooks");
    {
        let _span = operation.measure_thread().iterations(MEASURED_OPERATIONS as u64);
        for value in 0..MEASURED_OPERATIONS {
            tasks
                .task(async {})
                .poll_with(OneBytePollHook(value as u8))
                .register(CountCompletion(Rc::clone(&completed)));
            let _ = black_box(executor.execute_cycle());
        }
    }

    let (allocations, bytes) = allocation_totals(&session, "concrete_poll_hooks");
    assert_eq!(allocations, 0, "concrete optional poll hooks must not allocate");
    assert_eq!(bytes, 0, "concrete optional poll hooks must not allocate bytes");
    assert_eq!(completed.get(), MEASURED_OPERATIONS);
}
