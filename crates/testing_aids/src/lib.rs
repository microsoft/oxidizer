// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An unpublished crate containing testing utilities for use within this repo.

#![allow(clippy::panic, clippy::unwrap_used, missing_docs, reason = "Test code")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::time::Duration;
use std::{env, process, thread};

mod log;
mod macros;
mod metrics;
mod yielding;

pub use log::*;
pub use metrics::*;
pub use yielding::*;

/// If something (whatever) does not happen in a test within this time, the test will fail.
///
/// We are conservative here and allow much time - this is only to break out of infinite loops, not for any
/// situations that are actually expected.
///
/// This should be significantly smaller than the .cargo/mutants.toml timeout because multiple
/// tests may be executed during a single cargo-mutants run, so this timeout might not start
/// immediately at the start of a test run.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[must_use]
pub fn is_mutation_testing() -> bool {
    env::var("MUTATION_TESTING").as_deref() == Ok("1")
}

/// Executes a thread-safe function on a background thread and abandons it if
/// it does not complete before the provided timeout.
///
/// # Panics
///
/// Panics if the test panics or the test timeout is exceeded.
#[must_use]
pub fn execute_or_abandon<F, R>(f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if is_mutation_testing() {
        // Test timeouts are disabled under mutation testing - we want them to result in
        // actual "timeout" mutation test results. The idea is that such mutations need to
        // be skipped or tests improved to catch them - timeouts must not be
        // considered "normal" in mutation tests, as that wastes precious time.
        return Some(f());
    }

    let (sender, receiver) = mpsc::channel();

    // There are multiple ways for the called function to fail:
    // 1. It fails to finish in the allowed time span.
    // 2. It panics, so the result is never sent.
    //
    // In both cases, the channel will get closed and recv_timeout
    // will signal an error saying the channel is broken.
    thread::spawn(move || {
        let result = f();
        sender.send(result).unwrap();
    });

    receiver.recv_timeout(TEST_TIMEOUT).ok()
}

/// Executes a function on the current thread and sets up a watchdog timer that terminates the
/// process if the target function does not complete before the provided timeout.
///
/// This is a variant of `execute_or_abandon()` that can be used with single-threaded
/// logic that does not support being moved to a background thread.
///
/// # Panics
///
/// Panics if the test panics or the test timeout is exceeded.
pub fn execute_or_terminate_process<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    if is_mutation_testing() {
        // Test timeouts are disabled under mutation testing - we want them to result in
        // actual "timeout" mutation test results. The idea is that such mutations need to
        // be skipped or tests improved to catch them - timeouts must not be
        // considered "normal" in mutation tests, as that wastes precious time.
        return f();
    }

    let (sender, receiver) = mpsc::channel();

    let watchdog = thread::Builder::new()
        .name("test watchdog".to_string())
        .spawn(move || {
            if receiver.recv_timeout(TEST_TIMEOUT) == Ok(()) {
            } else {
                eprintln!("Test timed out, terminating process.");
                #[expect(
                    clippy::exit,
                    reason = "test harness is intentionally terminating test process that cannot continue execution"
                )]
                // Arbitrary value in portable range (8 bits) to signal "emergency timeout".
                process::exit(112);
            }
        })
        .unwrap();

    let result = catch_unwind(AssertUnwindSafe(f));

    // We signal "done" no matter whether it panics or succeeds, all we care about is timeout.
    sender.send(()).unwrap();

    // We must wait for this to finish, otherwise Miri leak detection will be angry at us.
    watchdog.join().unwrap();

    // This will re-raise any panic if one occurred.
    result.unwrap()
}

/// Standard test data generator - a repeating sequence of bytes from 0 to 255.
pub fn repeating_incrementing_bytes() -> impl Iterator<Item = u8> {
    (0..=u8::MAX).cycle()
}

/// Standard test data generator - a repeating sequence of bytes from 255 to 0.
pub fn repeating_reverse_incrementing_bytes() -> impl Iterator<Item = u8> {
    (0..=u8::MAX).rev().cycle()
}

/// Executes an async function on the Miri-compatible `futures` async task runtime,
/// blocking until it completes and enforcing a test timeout.
pub fn async_test<F, FF>(f: F)
where
    F: FnOnce() -> FF + 'static,
    FF: Future<Output = ()>,
{
    execute_or_terminate_process(|| {
        ::futures::executor::block_on(f());
    });
}
