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

use std::ffi::OsString;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(not(test))]
use std::process;
use std::sync::mpsc;
use std::time::Duration;
use std::{env, thread};

#[cfg(test)]
use test_hooks as process;

mod io;
mod macro_expansion;
mod macros;
mod metrics;
#[cfg(test)]
mod test_hooks;
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
    #[cfg(test)]
    if let Some(mode) = test_hooks::mode() {
        return mode;
    }
    is_mutation_testing_with(|name| env::var_os(name))
}

fn is_mutation_testing_with(get_env: impl Fn(&str) -> Option<OsString>) -> bool {
    get_env("CARGO_GAMMA").is_some() || mutation_testing_from(|name| get_env(name).and_then(|value| value.into_string().ok()))
}

fn mutation_testing_from(environment: impl FnOnce(&str) -> Option<String>) -> bool {
    environment("MUTATION_TESTING").as_deref() == Some("1")
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
    execute_or_abandon_mode(f, is_mutation_testing())
}

fn execute_or_abandon_mode<F, R>(f: F, mutation_mode: bool) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    execute_or_abandon_with(f, || mutation_mode, run_background)
}

fn run_background<F, R>(f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    #[cfg(test)]
    if test_hooks::background_requested() {
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

fn execute_or_abandon_with<F, R>(f: F, mutation_mode: impl FnOnce() -> bool, background: impl FnOnce(F) -> Option<R>) -> Option<R>
where
    F: FnOnce() -> R,
{
    if mutation_mode() {
        // Test timeouts are disabled under mutation testing - we want them to result in
        // actual "timeout" mutation test results. The idea is that such mutations need to
        // be skipped or tests improved to catch them - timeouts must not be
        // considered "normal" in mutation tests, as that wastes precious time.
        return Some(f());
    }

    background(f)
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
    execute_or_terminate_with(f, is_mutation_testing, || {
        let (sender, receiver) = mpsc::channel();

        let watchdog = start_watchdog(receiver);

        (move || sender.send(()).unwrap(), move || watchdog.join())
    })
}

enum Watchdog {
    Native(thread::JoinHandle<()>),
    #[cfg(test)]
    Injected(mpsc::Receiver<()>),
}

impl Watchdog {
    fn join(self) {
        match self {
            // #[gamma::skip(call_result.default, tag = "equivalent", reason = "Joining returns unit; default_after still joins and propagates a watchdog panic.")]
            Self::Native(watchdog) => join_watchdog(watchdog),
            #[cfg(test)]
            Self::Injected(receiver) => {
                receiver.try_recv().unwrap();
                test_hooks::record_watchdog_join();
            }
        }
    }
}

fn start_watchdog(receiver: mpsc::Receiver<()>) -> Watchdog {
    #[cfg(test)]
    if test_hooks::watchdog_is_injected() {
        return Watchdog::Injected(receiver);
    }
    Watchdog::Native(
        watchdog_builder()
            .spawn(move || {
                let completion = receiver.recv_timeout(TEST_TIMEOUT);
                // #[gamma::skip(call_result.default, tag = "equivalent", reason = "Completion returns unit; default_after preserves timeout handling and termination.")]
                // #[gamma::skip(call.replace_with_default, stmt.delete_call, reason = "This native watchdog thread can terminate the test process on timeout; complete_watchdog tests exercise success, disconnect and timeout through an injected termination probe")]
                complete_watchdog(completion);
            })
            .unwrap(),
    )
}

fn complete_watchdog(completion: Result<(), mpsc::RecvTimeoutError>) {
    // #[gamma::skip(call_result.default, reason = "finish_watchdog returns unit; default_after retains the timeout decision and exit side effects.")]
    finish_watchdog(completion, |code| terminate_watchdog(code));
}

#[cfg_attr(
    not(test),
    expect(clippy::exit, reason = "the emergency watchdog must terminate a test process that cannot continue")
)]
fn terminate_watchdog(code: i32) -> ! {
    process::exit(code);
}

fn watchdog_builder() -> thread::Builder {
    thread::Builder::new().name("test watchdog".to_string())
}

fn join_watchdog(watchdog: thread::JoinHandle<()>) {
    watchdog.join().unwrap();
}

fn execute_or_terminate_with<F, R, S, J>(f: F, mutation_mode: impl FnOnce() -> bool, start_watchdog: impl FnOnce() -> (S, J)) -> R
where
    F: FnOnce() -> R,
    S: FnOnce(),
    J: FnOnce(),
{
    if mutation_mode() {
        // Test timeouts are disabled under mutation testing - we want them to result in
        // actual "timeout" mutation test results. The idea is that such mutations need to
        // be skipped or tests improved to catch them - timeouts must not be
        // considered "normal" in mutation tests, as that wastes precious time.
        return f();
    }

    let (signal, join) = start_watchdog();

    let result = catch_unwind(AssertUnwindSafe(f));

    // We signal "done" no matter whether it panics or succeeds, all we care about is timeout.
    signal();

    // We must wait for this to finish, otherwise Miri leak detection will be angry at us.
    join();

    // This will re-raise any panic if one occurred.
    result.unwrap()
}

fn finish_watchdog(completion: Result<(), mpsc::RecvTimeoutError>, terminate: impl FnOnce(i32)) {
    if completion == Ok(()) {
    } else {
        eprintln!("Test timed out, terminating process.");
        terminate(112);
    }
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
        // #[gamma::skip(call_result.default, reason = "run_async_test returns unit; default_after still constructs and polls the supplied future.")]
        run_async_test(f);
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

fn run_async_test<F, FF>(f: F)
where
    F: FnOnce() -> FF,
    FF: Future<Output = ()>,
{
    ::futures::executor::block_on(f());
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod gamma_round2_tests {
    #[test]
    fn nov4_public_mode_query_and_abandon_routing_use_the_injected_environment() {
        for mode in [false, true] {
            let count = crate::test_hooks::with_background(|| {
                crate::test_hooks::with_mode(mode, || {
                    assert_eq!(super::is_mutation_testing(), mode);
                    assert_eq!(super::execute_or_abandon(|| 79), Some(79));
                });
            });
            assert_eq!(count, usize::from(!mode));
        }
    }

    #[test]
    fn nov4_public_watchdog_wrapper_signals_and_joins_the_injected_watchdog() {
        let joined = crate::test_hooks::with_watchdog(|| {
            crate::test_hooks::with_mode(false, || {
                assert_eq!(super::execute_or_terminate_process(|| 83), 83);
            });
        });
        assert_eq!(joined, 1);
    }

    #[test]
    fn nov4_watchdog_completion_selects_native_termination_only_for_failure_with_code_112() {
        super::complete_watchdog(Ok(()));
        for completion in [
            Err(super::mpsc::RecvTimeoutError::Timeout),
            Err(super::mpsc::RecvTimeoutError::Disconnected),
        ] {
            assert_eq!(crate::test_hooks::with_exit_probe(|| super::complete_watchdog(completion)), 112);
        }
    }
    #[test]
    #[serial_test::serial]
    fn nov4_mutation_mode_runs_the_callback_on_the_calling_thread_without_a_watchdog() {
        let owner = std::thread::current().id();
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = std::sync::Arc::clone(&runs);
        assert_eq!(
            super::execute_or_abandon_mode(
                move || {
                    assert_eq!(std::thread::current().id(), owner);
                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    79
                },
                true,
            ),
            Some(79)
        );
        assert_eq!(runs.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_native_watchdog_join_observes_an_actual_thread_failure_instead_of_detaching_it() {
        let watchdog = std::thread::spawn(|| panic!("round2 controlled watchdog failure"));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| super::Watchdog::Native(watchdog).join()));
        _ = result.unwrap_err();
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_native_abandon_wrapper_spawns_and_delivers_the_actual_thread_result() {
        let owner = std::thread::current().id();
        let (sent, received) = std::sync::mpsc::channel();
        let result = super::execute_or_abandon_mode(
            move || {
                let executing = std::thread::current().id();
                sent.send(executing).unwrap();
                (executing, 47)
            },
            false,
        );
        let executing = received.try_recv().unwrap();
        assert_ne!(executing, owner);
        assert_eq!(result, Some((executing, 47)));
    }

    use std::cell::{Cell, RefCell};
    use std::task::Poll;

    use super::*;

    #[test]
    #[serial_test::serial]
    fn gamma_round2_abandon_decision_runs_exactly_one_foreground_or_background_path() {
        for mutation_mode in [false, true] {
            let runs = Cell::new(0);
            let background = Cell::new(0);
            let result = execute_or_abandon_with(
                || {
                    runs.set(runs.get() + 1);
                    37
                },
                || mutation_mode,
                |f| {
                    background.set(background.get() + 1);
                    Some(f())
                },
            );
            assert_eq!(result, Some(37));
            assert_eq!(runs.get(), 1);
            assert_eq!(background.get(), usize::from(!mutation_mode));
        }
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_watchdog_orchestration_signals_then_joins_after_success_or_panic() {
        assert!(format!("{:?}", watchdog_builder()).contains("test watchdog"));
        for mutation_mode in [false, true] {
            for panics in [false, true] {
                let operations = RefCell::new(Vec::new());
                let result = catch_unwind(AssertUnwindSafe(|| {
                    execute_or_terminate_with(
                        || {
                            operations.borrow_mut().push("run");
                            assert!(!panics, "round2 task panic");
                            41
                        },
                        || mutation_mode,
                        || {
                            operations.borrow_mut().push("start");
                            (|| operations.borrow_mut().push("signal"), || operations.borrow_mut().push("join"))
                        },
                    )
                }));
                if panics {
                    let panic = result.unwrap_err();
                    if mutation_mode {
                        assert_eq!(panic.downcast_ref::<&str>(), Some(&"round2 task panic"));
                    } else {
                        assert_eq!(
                            panic.downcast_ref::<String>().unwrap(),
                            "called `Result::unwrap()` on an `Err` value: Any { .. }"
                        );
                    }
                } else {
                    assert_eq!(result.unwrap(), 41);
                }
                assert_eq!(
                    operations.into_inner(),
                    if mutation_mode {
                        vec!["run"]
                    } else {
                        vec!["start", "run", "signal", "join"]
                    }
                );
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_mutation_mode_parses_only_one_from_the_exact_named_variable() {
        for value in [None, Some(""), Some("0"), Some("1"), Some("xyzzy"), Some(" 1"), Some("1 ")] {
            assert_eq!(
                mutation_testing_from(|name| {
                    assert_eq!(name, "MUTATION_TESTING");
                    value.map(str::to_owned)
                }),
                value == Some("1")
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_watchdog_terminates_only_unsuccessful_completion_with_the_exact_code() {
        for completion in [
            Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected),
        ] {
            let code = Cell::new(None);
            finish_watchdog(completion, |value| {
                assert!(code.replace(Some(value)).is_none());
            });
            assert_eq!(code.get(), if completion.is_ok() { None } else { Some(112) });
        }
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_async_execution_constructs_and_polls_through_pending_to_completion() {
        let constructed = Cell::new(false);
        let polls = Cell::new(0);
        let result = Cell::new(0);
        run_async_test(|| {
            constructed.set(true);
            futures::future::poll_fn(|cx| {
                let count = polls.get() + 1;
                polls.set(count);
                if count == 1 {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    result.set(37);
                    Poll::Ready(())
                }
            })
        });
        assert!(constructed.get());
        assert_eq!(polls.get(), 2);
        assert_eq!(result.get(), 37);
    }
}
