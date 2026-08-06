// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates what happens when an uncaught panic escapes from an async task.
//!
//! An uncaught panic terminates the process as the most conservative approach, as the executor is
//! not designed with any panic handling mechanisms today.
//!
//! Higher levels of the Arty runtime wrap the tasks in panic-handling layers, so this sort of
//! shutdown should never be seen by users of Arty runtime - it is only something that can
//! occur when using the executor directly, without a panic-handling layer inside the tasks.

use arty_executor::Executor;

#[expect(clippy::panic, reason = "the example demonstrates executor behavior when a task panics")]
fn main() {
    // SAFETY: We are required to complete safe shutdown of the executor by only dropping it once
    // an execution cycle indicates the `Shutdown` outcome. We do.
    let executor = unsafe { Executor::builder().build() };
    let tasks = executor.tasks();

    tasks.add(async move {
        panic!("Panic from the async task!");
    });

    _ = executor.execute_cycle();

    unreachable!("The process should have terminated by now due to the panic.");
}
