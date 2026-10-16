// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression tests for the `write_to_stdout_and_buffer` capture bridge.
//!
//! These live in their own integration-test binary and every test is `#[serial]`
//! because capture is process-global. They verify the load-bearing property of the
//! bridge: capture works deterministically regardless of test execution order,
//! including after a callsite has been reached while output was silent.

use std::cell::Cell;
use std::fmt;

use serial_test::serial;
use testing_aids::tracing_logs::{log_file, write_to_stdout_and_buffer, write_to_stdout_and_file};

// The capture bridge asserts the subscriber was installed at process start, so install it
// here. Integration binaries do not run any crate-root `#[cfg(test)]` initialization. See
// docs/tracing-tests.md.
testing_aids::init_tracing!();

#[test]
#[serial]
fn captures_emitted_lines() {
    let guard = write_to_stdout_and_buffer();
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
    let guard = write_to_stdout_and_buffer();
    tracing::info!(marker = "first_scope", "an event");
    let _ = guard.into_inner();

    // Emitting outside any capture scope must not accumulate anywhere.
    tracing::info!(marker = "between_scopes", "an event");

    // Second capture scope must only see its own events.
    let guard = write_to_stdout_and_buffer();
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
fn capture_works_after_prior_debug_emission_without_capture() {
    // Deliberately emit at a fresh callsite BEFORE any capture buffer is enabled
    // (the silent fallback subscriber is already installed by the file-level ctor).
    // Without an always-interested subscriber, `tracing-core`'s fast path could cache
    // this callsite's interest as "disabled" process-wide and permanently, suppressing
    // all later capture at this callsite. The always-interested formatting layer in the
    // global fallback subscriber must prevent that.
    poison_attempt();

    let guard = write_to_stdout_and_buffer();
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
    let guard = write_to_stdout_and_buffer();

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

#[test]
#[serial]
fn formats_once_with_stdout_and_buffer_enabled() {
    let guard = write_to_stdout_and_buffer();
    let formats = Cell::new(0);
    let value = FormatProbe(&formats);
    let span = tracing::trace_span!("captured span", initial = ?value, recorded = tracing::field::Empty);
    assert_eq!(formats.get(), 1);
    span.record("recorded", tracing::field::debug(&value));
    assert_eq!(formats.get(), 2);
    let _entered = span.enter();
    tracing::info!(debug = ?value, display = %value, "message {value}");
    assert_eq!(formats.get(), 5);
    let lines = guard.into_inner();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("initial=formatted recorded=formatted"));
    assert!(lines[0].contains("debug=formatted display=formatted"));
}

#[test]
#[serial]
#[cfg_attr(
    miri,
    ignore = "file output is covered by native tests and cargo careful; Miri retains buffer routing tests"
)]
fn formats_once_with_stdout_file_and_buffer_enabled() {
    let file_name = "testing-aids-formatting.log";
    let file_guard = write_to_stdout_and_file(file_name);
    let buffer_guard = write_to_stdout_and_buffer();
    let formats = Cell::new(0);
    let value = FormatProbe(&formats);
    tracing::info!(?value, "info-marker");
    tracing::debug!(?value, "debug-marker");
    tracing::trace!(?value, "trace-marker");
    assert_eq!(formats.get(), 3);
    drop(file_guard);
    tracing::info!("after-file-marker");
    let lines = buffer_guard.into_inner();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("info-marker"));
    assert!(lines[1].contains("debug-marker"));
    assert!(lines[2].contains("trace-marker"));
    assert!(lines[3].contains("after-file-marker"));

    if !testing_aids::is_mutation_testing() {
        let path = log_file(file_name);
        let file_output = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(file_output, format!("{}\n", lines[..3].join("\n")));
    }
}

#[test]
#[serial]
fn captures_span_fields_recorded_before_capture_begins() {
    let formats = Cell::new(0);
    let value = FormatProbe(&formats);
    let span = tracing::trace_span!("early span", initial = ?value, recorded = tracing::field::Empty);
    span.record("recorded", tracing::field::debug(&value));
    assert_eq!(formats.get(), 2);

    let guard = write_to_stdout_and_buffer();
    let _entered = span.enter();
    tracing::trace!("late-event-marker");
    let lines = guard.into_inner();
    assert_eq!(formats.get(), 2);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("initial=formatted recorded=formatted"));
    assert!(lines[0].contains("late-event-marker"));
}

#[test]
#[serial]
fn global_capture_discards_partial_events_after_formatting_panics() {
    struct PanickingFormat;

    impl fmt::Debug for PanickingFormat {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("partial-format-marker")?;
            panic!("formatting panic");
        }
    }

    let guard = write_to_stdout_and_buffer();
    testing_aids::assert_panic!(tracing::trace!(value = ?PanickingFormat));
    assert!(guard.snapshot().is_empty());
    tracing::trace!("complete-event-marker");
    let lines = guard.into_inner();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("complete-event-marker"));
    assert!(!lines[0].contains("partial-format-marker"));
}

#[test]
#[serial]
fn formatting_can_emit_nested_events_and_inspect_capture() {
    struct NestedFormat<'a>(&'a testing_aids::tracing_logs::BufferGuard);

    impl fmt::Debug for NestedFormat<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            tracing::trace!("nested-event-marker");
            assert_eq!(self.0.snapshot().len(), 1);
            f.write_str("nested-formatted")
        }
    }

    let guard = write_to_stdout_and_buffer();
    tracing::trace!(value = ?NestedFormat(&guard), "outer-event-marker");
    let lines = guard.into_inner();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("nested-event-marker"));
    assert!(!lines[0].contains("outer-event-marker"));
    assert!(lines[1].contains("outer-event-marker"));
    assert!(!lines[1].contains("nested-event-marker"));
}

struct FormatProbe<'a>(&'a Cell<usize>);

impl fmt::Debug for FormatProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.set(self.0.get() + 1);
        f.write_str("formatted")
    }
}

impl fmt::Display for FormatProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

// A single, unique callsite reused by the poisoning regression test so that the
// "before" and "after" emissions share the exact same cached interest entry.
fn poison_attempt() {
    tracing::debug!(marker = "poison_probe", "probe event");
}
