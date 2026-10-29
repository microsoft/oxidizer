// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs a single asynchronous task on the async task executor, as the minimal scenario.

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

    // When our single task is done, it will set this signal to indicate that the app can exit.
    let shutdown_signal = Rc::new(Cell::new(false));

    tasks.add(
        {
            let shutdown_signal = Rc::clone(&shutdown_signal);

            async move {
                println!("Hello from the async task!");

                // Nothing more we need to do now.
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

            // No need to yield - we can immediately process the shutdown.
            continue;
        }

        // A real app framework would do some useful work here, such as processing I/O and timers.
        thread::yield_now();
    }
}
