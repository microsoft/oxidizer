// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Test-only helper crate: its utilities are exercised opportunistically by
// other crates' tests, so per-package line coverage is not a meaningful gate
// here. Exclude the whole crate from instrumentation (mirrors the codecov.yml
// `ignore` this replaces).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! An unpublished crate containing testing utilities for use within this repo.

#![allow(clippy::panic, clippy::unwrap_used, missing_docs, reason = "Test code")]

use std::ffi::{OsStr, OsString};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::time::Duration;
use std::{env, process, thread};

mod io;
mod macro_expansion;
mod macros;
mod metrics;
pub mod tracing_logs;
mod yielding;

pub use io::*;
pub use macro_expansion::*;
pub use metrics::*;
pub use yielding::*;

#[cfg(test)]
crate::init_tracing!();

/// If something (whatever) does not happen in a test within this time, the test will fail.
///
/// We are conservative here and allow much time - this is only to break out of infinite loops, not for any
/// situations that are actually expected.
///
/// Mutation runs disable this watchdog so Gamma can report its own timeout verdict.
/// Ordinary runs use it to stop a test that makes no progress.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[must_use]
pub fn is_mutation_testing() -> bool {
    is_mutation_testing_with(|name| env::var_os(name))
}

fn is_mutation_testing_with(get_env: impl Fn(&str) -> Option<OsString>) -> bool {
    get_env("CARGO_GAMMA").is_some() || get_env("MUTATION_TESTING").as_deref() == Some(OsStr::new("1"))
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::is_mutation_testing_with;

    #[test]
    fn mutation_testing_recognizes_gamma_and_legacy_opt_in() {
        for (legacy, gamma, expected) in [
            (None, None, false),
            (Some("0"), None, false),
            (Some("1"), None, true),
            (None, Some("1"), true),
            (Some("0"), Some(""), true),
        ] {
            let actual = is_mutation_testing_with(|name| {
                let value = match name {
                    "MUTATION_TESTING" => legacy,
                    "CARGO_GAMMA" => gamma,
                    _ => panic!("unexpected environment variable: {name}"),
                };
                value.map(OsString::from)
            });
            assert_eq!(actual, expected, "MUTATION_TESTING={legacy:?}, CARGO_GAMMA={gamma:?}");
        }
    }
}
