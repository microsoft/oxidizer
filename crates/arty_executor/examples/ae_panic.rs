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

use std::env;
use std::process::Command;

use arty_executor::Executor;

const EXPECTED_FAILURE_CHILD_ENV: &str = "ARTY_EXECUTOR_EXPECTED_FAILURE_CHILD";

fn main() {
    if env::var_os("IS_TESTING").is_some() && env::var_os(EXPECTED_FAILURE_CHILD_ENV).is_none() {
        assert_failure_in_child();
    } else {
        run_example();
    }
}

fn assert_failure_in_child() {
    let executable = env::current_exe().expect("the running example necessarily has a current executable path");
    let status = Command::new(executable)
        .env(EXPECTED_FAILURE_CHILD_ENV, "1")
        .status()
        .expect("the example executable that is already running must be startable as a child");

    assert!(!status.success(), "the child example must terminate after the task panic");
}

#[expect(clippy::panic, reason = "the example demonstrates executor behavior when a task panics")]
fn run_example() {
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
