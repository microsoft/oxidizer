// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Formatting preserves field labels and propagates sink write failures.

use std::fmt::{self, Write};
use std::time::SystemTime;

use observed::{FlushError, Sink, SinkFlushError, SinkId, Text, Value};

#[test]
fn gamma_survivor_erased_redacted_value_uses_static_empty_text() {
    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use data_privacy::{DataClass, Sensitive};

    let value = Sensitive::new("secret", DataClass::new("gamma", "erased"));
    let redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
    assert!(matches!(Value::from_redacted(&value, &redactor), Value::String(Text::Static(""))));
}

struct FailAfter(usize);

impl Write for FailAfter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if s.len() > self.0 {
            return Err(fmt::Error);
        }
        self.0 -= s.len();
        Ok(())
    }
}

#[test]
fn gamma_survivor_array_formatting_returns_errors_from_bracket_separator_and_element() {
    let value = Value::from(vec![11_i64, 22]);
    assert_eq!(value.to_string(), "[11, 22]");
    for budget in [0, 1, 3, 5, 7] {
        assert_eq!(write!(&mut FailAfter(budget), "{value}"), Err(fmt::Error));
    }
    let mut output = String::new();
    write!(&mut output, "{value}").unwrap();
    assert_eq!(output, "[11, 22]");
}

#[test]
fn gamma_survivor_flush_error_propagates_header_and_failure_line_write_errors() {
    let error = SinkFlushError::from_failures(vec![
        FlushError::new("one", std::io::Error::other("offline")),
        FlushError::new("two", std::io::Error::other("broken")),
    ]);
    let text = error.to_string();
    assert!(text.contains("2 processors failed to flush:"));
    assert!(text.contains("offline"));
    assert!(text.contains("broken"));
    for budget in [0, "2 processors failed to flush:".len()] {
        assert_eq!(write!(&mut FailAfter(budget), "{error}"), Err(fmt::Error));
    }
}

#[test]
fn gamma_survivor_sink_debug_preserves_variant_and_field_names() {
    let id = SinkId::new("gamma_debug");
    assert_eq!(format!("{id:?}"), "observed::sink::id::SinkId { label: \"gamma_debug\" }");
    let clock = tick::SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH);
    let builder = Sink::builder(id).clock(clock);
    let builder_debug = format!("{builder:?}");
    assert!(builder_debug.contains("id: "));
    assert!(builder_debug.contains("clock: "));
    let sink = builder.build();
    assert_eq!(
        format!("{sink:?}"),
        "observed::sink::core::Sink { variant: \"Single\", id: observed::sink::id::SinkId { label: \"gamma_debug\" }, processors: 0, isolated_enrichment: false }"
    );
    let other = Sink::builder("other")
        .clock(tick::SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH))
        .build();
    let composite = Sink::composite([sink, other]);
    assert_eq!(
        format!("{composite:?}"),
        "observed::sink::core::Sink { variant: \"Composite\", children: 2 }"
    );
    assert_eq!(format!("{:?}", Sink::noop()), "observed::sink::core::Sink { variant: \"Noop\" }");
}
