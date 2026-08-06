// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs a single asynchronous task on the async task executor, as the minimal scenario.

use std::cell::Cell;
use std::rc::Rc;
use std::thread;

use arty_executor::{CycleOutcome, Executor};

fn main() {
    // SAFETY: We are required to complete safe shutdown of the executor by only dropping it once
    // an execution cycle indicates the `Shutdown` outcome. We do.
    let executor = unsafe { Executor::builder().build() };
    let tasks = executor.tasks();

    // When our single task is done, it will set this signal to indicate that the app can exit.
    let shutdown_signal = Rc::new(Cell::new(false));

    tasks.add({
        let shutdown_signal = Rc::clone(&shutdown_signal);

        async move {
            println!("Hello from the async task!");

            // Nothing more we need to do now.
            shutdown_signal.set(true);
        }
    });

    let mut shutdown_started = false;

    while executor.execute_cycle() != CycleOutcome::Shutdown {
        if !shutdown_started && shutdown_signal.get() {
            shutdown_started = true;

            executor.begin_shutdown();

            // No need to yield - we can immediately process the shutdown.
            continue;
        }

        // A real app framework would do some useful work here, such as processing I/O and timers.
        thread::yield_now();
    }
}
