// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs one async task and then shuts down the executor.

use std::cell::Cell;
use std::rc::Rc;
use std::thread;

use arty_executor::{CycleOutcome, Executor, ShutdownCause, TaskCompletion, TaskOptions, TaskPanicPayload, discard_task_panic_payload};

/// Supplies completion callbacks without a result receiver.
///
/// Successful results and cancellation need no action. Panic payloads still
/// require explicit disposal.
#[derive(Debug)]
struct DiscardCompletion;

impl TaskCompletion<()> for DiscardCompletion {
    fn completed(self, _result: ()) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

fn main() {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();

    // The task sets this signal when the application can start shutdown.
    let shutdown_signal = Rc::new(Cell::new(false));

    tasks.add(
        {
            let shutdown_signal = Rc::clone(&shutdown_signal);

            async move {
                println!("Hello from the async task!");

                shutdown_signal.set(true);
            }
        },
        TaskOptions::default(),
        DiscardCompletion,
    );

    let mut shutdown_started = false;

    while !matches!(executor.execute_cycle(), CycleOutcome::Shutdown(_)) {
        if !shutdown_started && shutdown_signal.get() {
            shutdown_started = true;

            executor.shutdown().trigger(arty_executor::ShutdownCause::default());

            // Process shutdown immediately.
            continue;
        }

        // An application can process I/O and timers here.
        thread::yield_now();
    }
}
