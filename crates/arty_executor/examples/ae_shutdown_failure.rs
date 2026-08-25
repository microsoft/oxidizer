// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates what happens when the executor fails to shut down due to a resource leak.
//!
//! Such resource leaks can be intentional or accidental - it is perfectly legal for something to
//! hold on to executor resources even during/after executor shutdown for a while. However, if the
//! owner of the executor has requested that the executor shut down, the expectation is that the
//! holders of those resources are similarly shutting down and will release the resources quickly.
//!
//! If that does not happen, the executor cannot complete its shutdown process because it would
//! violate memory safety if it did that. Therefore, we try to report what happened and terminate
//! the process.
//!
//! When executing this in a debug build and with `RUST_BACKTRACE=1` set, the executor will log
//! the maximum level of diagnostic information before terminating the process.

use std::cell::Cell;
use std::future::poll_fn;
use std::pin::Pin;
use std::process::Command;
use std::rc::Rc;
use std::task::Waker;
use std::time::Duration;
use std::{env, task, thread};

use arty_executor::{CycleOutcome, Executor};
use tracing::info;

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

    assert!(!status.success(), "the child example must terminate after shutdown times out");
}

fn run_example() {
    tracing_subscriber::fmt::init();

    let mut executor_builder = Executor::builder();
    if env::var_os("IS_TESTING").is_some() {
        executor_builder = executor_builder.shutdown_timeout(Duration::ZERO);
    }
    // SAFETY: We are required to complete safe shutdown of the executor by only dropping it once
    // an execution cycle indicates the `Shutdown` outcome. We do.
    let executor = unsafe { executor_builder.build() };
    let tasks = executor.tasks();

    // When our single task is done, it will set this signal to indicate that the app can exit.
    let shutdown_signal = Rc::new(Cell::new(false));

    // We simulate an `.await` that was never canceled and so it kept the waker. This may have been
    // some kind of `Delay(15 minutes).await`, so maybe one day it completes but that is too late.
    //
    // Responsibilities of executor owner to avoid resource leak: nothing - this indicates a defect
    // in the implementation of the thing being awaited because it should cancel the await when the
    // future is dropped, thereby releasing resources.
    let leaked_waker = Rc::new(Cell::new(None));

    // We simulate a `JoinHandle` leak by simply keeping the join handle of the first task around.
    // This may have been a join handle passed to a custom thread or one passed to a different
    // executor - fairly untypical scenarios but they can happen in complex apps.
    //
    // Responsibilities of executor owner to avoid resource leak: ensure that anything holding a
    // join handle will drop it when shutdown starts.
    let mut leaked_join_handle = tasks.add({
        let shutdown_signal = Rc::clone(&shutdown_signal);
        let leaked_waker = Rc::clone(&leaked_waker);

        async move {
            // This is our simulated "Delay(15 minutes).await" call.
            //
            // In the backtrace emitted to the log, you will see "poll_fn" named.
            poll_fn(|cx| {
                leaked_waker.set(Some(cx.waker().clone()));
                // We return Ready here simply to make the example progress. The impact of
                // the resource leak is not affected by the return value of this poll operation.
                task::Poll::Ready(())
            })
            .await;

            // Nothing more we need to do now.
            shutdown_signal.set(true);
        }
    });

    // We poll the join handle once to simulate us awaiting it. This is not required for it to be
    // a resource leak - merely holding a join handle and never awaiting it will also block the
    // shutdown process. Awaited join handles can provide extra debug information in logs, though.
    //
    // In the backtrace emitted to the log, you will see `simulate_join_handle_await` named.
    simulate_join_handle_await(&mut leaked_join_handle);

    let mut shutdown_started = false;

    while executor.execute_cycle() != CycleOutcome::Shutdown {
        if !shutdown_started && shutdown_signal.get() {
            info!("Shutdown process is starting now. Please wait up to 60 seconds.");

            shutdown_started = true;

            executor.begin_shutdown();

            // No need to yield - we can immediately process the shutdown.
            continue;
        }

        // A real app framework would do some useful work here, such as processing I/O and timers.
        thread::yield_now();
    }

    // Ensure they are kept alive at least until this point.
    drop(leaked_join_handle);
    drop(leaked_waker);

    info!("Shutdown process completed successfully. This should never happen in this example.");
}

fn simulate_join_handle_await<R>(join_handle: &mut arty_executor::JoinHandle<R>) {
    // We simulate polling the join handle by just calling poll once.
    // This is not required for the leak to be present - merely holding the join handle is enough.
    let mut cx = task::Context::from_waker(Waker::noop());
    _ = Pin::new(join_handle).poll(&mut cx);

    // We do not care about the result, we just want to ensure that the join handle is polled at least once.
}
