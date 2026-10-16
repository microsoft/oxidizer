// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Formatting regressions in a binary that never enables process-global output.

use std::cell::Cell;
use std::fmt;

use testing_aids::tracing_logs::{Capture, initialize};
use tracing::{Level, field};
use tracing_subscriber::util::SubscriberInitExt;

testing_aids::init_tracing!();

struct FormatProbe<'a> {
    debug: &'a Cell<usize>,
    display: &'a Cell<usize>,
}

impl fmt::Debug for FormatProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.debug.set(self.debug.get() + 1);
        f.write_str("debug-formatted")
    }
}

impl fmt::Display for FormatProbe<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display.set(self.display.get() + 1);
        f.write_str("display-formatted")
    }
}

#[test]
fn silent_events_and_spans_exercise_expressions_and_formatting_at_every_level() {
    macro_rules! check_level {
        ($level:expr) => {{
            let expressions = Cell::new(0);
            let debug = Cell::new(0);
            let display = Cell::new(0);
            let value = FormatProbe {
                debug: &debug,
                display: &display,
            };

            tracing::event!(
                $level,
                expression = {
                    expressions.set(expressions.get() + 1);
                    42
                },
                debug = ?value,
                display = %value,
                "message {value}"
            );
            assert_eq!(expressions.get(), 1);
            assert_eq!(debug.get(), 1);
            assert_eq!(display.get(), 2);

            let span = tracing::span!(
                $level,
                "silent span",
                expression = {
                    expressions.set(expressions.get() + 1);
                    42
                },
                debug = ?value,
                display = %value,
                recorded_debug = field::Empty,
                recorded_display = field::Empty,
            );
            assert_eq!(expressions.get(), 2);
            assert_eq!(debug.get(), 2);
            assert_eq!(display.get(), 3);

            span.record("recorded_debug", field::debug(&value));
            span.record("recorded_display", field::display(&value));
            assert_eq!(debug.get(), 3);
            assert_eq!(display.get(), 4);
        }};
    }

    check_level!(Level::ERROR);
    check_level!(Level::WARN);
    check_level!(Level::INFO);
    check_level!(Level::DEBUG);
    check_level!(Level::TRACE);
}

#[test]
fn formatting_survives_reinitialization_and_thread_local_capture() {
    let debug = Cell::new(0);
    let display = Cell::new(0);
    let value = FormatProbe {
        debug: &debug,
        display: &display,
    };

    emit_probe(&value);
    assert_eq!(debug.get(), 1);
    initialize();
    initialize();

    let capture = Capture::new();
    {
        let _guard = capture.subscriber().set_default();
        emit_probe(&value);
        assert_eq!(debug.get(), 2);
        std::thread::spawn(|| {
            let debug = Cell::new(0);
            let display = Cell::new(0);
            emit_probe(&FormatProbe {
                debug: &debug,
                display: &display,
            });
            assert_eq!(debug.get(), 1);
        })
        .join()
        .unwrap();
    }

    emit_probe(&value);
    assert_eq!(debug.get(), 3);
    assert_eq!(capture.output().lines().count(), 1);
    capture.assert_contains("debug-formatted");
}

fn emit_probe(value: &FormatProbe<'_>) {
    tracing::trace!(?value, "reused callsite");
}

struct PanickingFormat;

impl fmt::Debug for PanickingFormat {
    #[expect(clippy::panic, reason = "regression test for propagating formatting failures")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("partial-format-marker")?;
        panic!("formatting panic");
    }
}

impl fmt::Display for PanickingFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[test]
fn silent_formatting_panics_propagate_without_leaking_partial_events() {
    testing_aids::assert_panic!(tracing::trace!(value = ?PanickingFormat));
    testing_aids::assert_panic!(tracing::debug!(value = %PanickingFormat));
    testing_aids::assert_panic!(tracing::info!("message {}", PanickingFormat));

    let capture = Capture::new();
    let _guard = capture.subscriber().set_default();
    tracing::trace!("complete-event-marker");
    let output = capture.output();
    assert_eq!(output.lines().count(), 1);
    assert!(output.contains("complete-event-marker"));
    assert!(!output.contains("partial-format-marker"));
}

#[test]
fn captured_formatting_panics_do_not_leak_partial_events() {
    let capture = Capture::new();
    let _guard = capture.subscriber().set_default();

    testing_aids::assert_panic!(tracing::trace!(value = ?PanickingFormat));
    assert!(capture.output().is_empty());
    tracing::trace!("complete-event-marker");
    let output = capture.output();
    assert_eq!(output.lines().count(), 1);
    assert!(output.contains("complete-event-marker"));
    assert!(!output.contains("partial-format-marker"));
}
