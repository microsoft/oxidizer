// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded formatting and writer-failure contracts.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::{self, Write as _};

use data_privacy::{DataClass, RedactedDisplay, RedactedToString, RedactionEngine, Sensitive};
use observed_utils::{SensitiveSlice, format_any_value};
use opentelemetry::Key;
use opentelemetry::logs::AnyValue;

#[test]
fn gamma_survivor_otel_text_conversions_retain_static_and_shared_backing_allocations() {
    use std::sync::Arc;

    use observed::{Text, Value};
    use observed_utils::{any_value_of, otel_value_of};

    let shared: Arc<str> = Arc::from("shared text");
    for text in [Text::Static("static text"), Text::Shared(Arc::clone(&shared))] {
        let backing = text.as_str().as_ptr();
        let AnyValue::String(converted) = any_value_of(Value::String(text.clone())) else {
            panic!("string conversion must preserve its value kind");
        };
        assert_eq!(converted.as_str().as_ptr(), backing);
        let opentelemetry::Value::String(converted) = otel_value_of(Value::String(text)) else {
            panic!("metric conversion must preserve its value kind");
        };
        assert_eq!(converted.as_str().as_ptr(), backing);
    }
}

struct FailingWriter {
    remaining: usize,
}

impl fmt::Write for FailingWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if s.len() > self.remaining {
            return Err(fmt::Error);
        }
        self.remaining -= s.len();
        Ok(())
    }
}

#[test]
fn gamma_survivor_recursive_output_propagates_each_write_error() {
    let values = [
        AnyValue::ListAny(Box::new(vec![AnyValue::Int(12), AnyValue::Int(34)])),
        AnyValue::Map(Box::new(HashMap::from([
            (Key::from("first"), AnyValue::Int(12)),
            (Key::from("second"), AnyValue::Int(34)),
        ]))),
    ];
    for value in values {
        let rendered = format_any_value(&value).to_string();
        for remaining in 0..rendered.len() {
            assert_eq!(
                write!(FailingWriter { remaining }, "{}", format_any_value(&value)),
                Err(fmt::Error),
                "budget {remaining} for {rendered}"
            );
        }
        write!(FailingWriter { remaining: rendered.len() }, "{}", format_any_value(&value)).unwrap();
    }
}

#[test]
fn gamma_survivor_sensitive_slice_stops_after_overflow_probe() {
    const CLASS: DataClass = DataClass::new("test", "value");
    let items = ["one", "two", "three", "four", "five"].map(|s| Sensitive::new(s, CLASS));
    let pulls = Cell::new(0);
    let slice = SensitiveSlice::<2, ';'>::new(items.iter().inspect(|_| pulls.set(pulls.get() + 1)));
    assert_eq!(pulls.get(), 3);
    let engine = RedactionEngine::builder().suppress_redaction(CLASS).build();
    assert_eq!(slice.to_redacted_string(&engine), "one; two; ...");
    for remaining in 0.."one; two; ...".len() {
        assert_eq!(
            write!(FailingWriter { remaining }, "{}", fmt::from_fn(|f| slice.fmt(&engine, f))),
            Err(fmt::Error)
        );
    }
}
