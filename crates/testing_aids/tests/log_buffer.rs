// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression tests for the `log_to_stdout_and_buffer` capture bridge.
//!
//! These live in their own integration-test binary and every test is `#[serial]`
//! because capture is process-global. They verify the load-bearing property of the
//! bridge: capture works deterministically regardless of test execution order, even
//! after a callsite has been reached with no subscriber installed (which would
//! otherwise poison `tracing-core`'s process-global callsite-interest cache).

use serial_test::serial;
use testing_aids::log_to_stdout_and_buffer;

// The capture bridge asserts the fallback was installed at process start, so install it
// here. Integration binaries do not run any crate-root `#[cfg(test)]` constructor. See
// docs/tracing-tests.md.
#[ctor::ctor(unsafe)]
fn init_test_tracing() {
    testing_aids::initialize_logging();
}

#[test]
#[serial]
fn captures_emitted_lines() {
    let guard = log_to_stdout_and_buffer();
    tracing::info!(marker = "hello_capture", "an event");
    let lines = guard.into_inner();

    assert!(
        lines.iter().any(|line| line.contains("hello_capture")),
        "expected captured lines to contain the emitted marker, got: {lines:?}"
    );
}

#[test]
#[serial]
fn capture_is_empty_after_guard_detaches() {
    // First capture scope: emit something.
    let guard = log_to_stdout_and_buffer();
    tracing::info!(marker = "first_scope", "an event");
    let _ = guard.into_inner();

    // Emitting outside any capture scope must not accumulate anywhere.
    tracing::info!(marker = "between_scopes", "an event");

    // Second capture scope must only see its own events.
    let guard = log_to_stdout_and_buffer();
    tracing::info!(marker = "second_scope", "an event");
    let lines = guard.into_inner();

    assert!(lines.iter().any(|line| line.contains("second_scope")));
    assert!(
        !lines.iter().any(|line| line.contains("between_scopes")),
        "capture leaked an event emitted outside any guard, got: {lines:?}"
    );
}

#[test]
#[serial]
fn capture_works_after_prior_debug_emission_without_subscriber() {
    // Deliberately emit at a fresh callsite BEFORE any subscriber is installed.
    // Under `tracing-core`'s fast path this would cache the callsite's interest as
    // "disabled" process-wide and permanently, suppressing all later capture at
    // this callsite. The always-interested buffer layer in the global subscriber
    // must prevent that.
    poison_attempt();

    let guard = log_to_stdout_and_buffer();
    poison_attempt();
    let lines = guard.into_inner();

    assert!(
        lines.iter().any(|line| line.contains("poison_probe")),
        "callsite interest was poisoned: capture saw nothing after a prior \
         no-subscriber emission, got: {lines:?}"
    );
}

#[test]
#[serial]
fn snapshot_reads_without_detaching() {
    let guard = log_to_stdout_and_buffer();

    tracing::info!(marker = "first_line", "an event");
    let first = guard.snapshot();
    assert!(first.iter().any(|line| line.contains("first_line")));

    // The guard is still active: a later emission must accumulate on top of what
    // an earlier snapshot already observed.
    tracing::info!(marker = "second_line", "an event");
    let lines = guard.into_inner();

    assert!(lines.iter().any(|line| line.contains("first_line")));
    assert!(lines.iter().any(|line| line.contains("second_line")));
}

// A single, unique callsite reused by the poisoning regression test so that the
// "before" and "after" emissions share the exact same cached interest entry.
fn poison_attempt() {
    tracing::debug!(marker = "poison_probe", "probe event");
}
