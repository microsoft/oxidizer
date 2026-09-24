// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared `OTel` log-record mapping for the `observed` crate's examples,
//! benchmark, and integration test.
//!
//! Turning an [`EventView`] into an `OTel` log record is correctness-sensitive:
//! it decides which fields reach the exporter, whether each value goes through
//! the processor's redaction engine, and which timestamp the record carries.
//! Eight scenarios needed that mapping and each had grown its own copy, so a
//! correction had to be rediscovered eight times - and the examples, which
//! consumers read as the recommended pattern, taught readers to reinvent it.
//!
//! Only the mapping lives here. Each scenario keeps its own processor type,
//! provider ownership, and lifecycle, because those are what the individual
//! example is demonstrating.
//!
//! Included via `#[path = "support/otel.rs"]` so the scenarios stay
//! self-contained and do not depend on the internal `observed_testing` harness.

use std::ops::ControlFlow;

use observed::metadata::LogDescription;
use observed::processing::EventView;
use observed::{Severity, Text, Value};
use opentelemetry::logs::{AnyValue, LogRecord};

/// `OTel` attribute key for the source file a call site came from.
const CODE_FILE_PATH: &str = "code.file.path";

/// `OTel` attribute key for the source line a call site came from.
const CODE_LINE_NUMBER: &str = "code.line.number";

/// Converts a [`Text`] into an `OTel` string, preserving the borrowed-versus-
/// shared distinction so neither representation copies.
///
/// `observed_utils` offers this conversion (and the two below) ready-made, but
/// it depends on `observed`, so using it here would make `observed`'s dependency
/// graph cyclic. A consumer outside this crate should call
/// `observed_utils::any_value_of` rather than copy this.
fn string_value_of(text: Text) -> opentelemetry::StringValue {
    match text {
        Text::Static(s) => s.into(),
        Text::Shared(s) => s.into(),
        ref other => other.as_str().to_owned().into(),
    }
}

/// Converts a [`Value`] into an `OTel` [`AnyValue`] for a log-record attribute.
fn any_value_of(value: Value) -> AnyValue {
    fn list_of<T>(values: Vec<T>, mut f: impl FnMut(T) -> AnyValue) -> AnyValue {
        AnyValue::ListAny(Box::new(values.into_iter().map(&mut f).collect()))
    }

    match value {
        Value::Bool(v) => AnyValue::Boolean(v),
        Value::I64(v) => AnyValue::Int(v),
        // Saturates at `i64::MAX`, matching `observed_utils::any_value_of`:
        // `AnyValue` has no unsigned variant, and the two must agree because the
        // doc above points readers at the utility instead of this copy.
        Value::U64(v) => AnyValue::Int(i64::try_from(v).unwrap_or(i64::MAX)),
        Value::F64(v) => AnyValue::Double(v),
        Value::String(v) => AnyValue::String(string_value_of(v)),
        Value::BoolArray(v) => list_of(v, AnyValue::Boolean),
        Value::I64Array(v) => list_of(v, AnyValue::Int),
        Value::F64Array(v) => list_of(v, AnyValue::Double),
        Value::StringArray(v) => list_of(v, |s| AnyValue::String(string_value_of(s))),
        other => AnyValue::String(format!("{other:?}").into()),
    }
}

/// Converts a [`Severity`] into its `OTel` counterpart.
fn otel_severity_of(severity: Severity) -> opentelemetry::logs::Severity {
    use opentelemetry::logs::Severity as Otel;

    match severity {
        Severity::Trace => Otel::Trace,
        Severity::Debug => Otel::Debug,
        Severity::Warn => Otel::Warn,
        Severity::Error => Otel::Error,
        Severity::Fatal => Otel::Fatal,
        // `Severity::Info`, plus the guard for the `#[non_exhaustive]` enum.
        _ => Otel::Info,
    }
}

/// Populates `record` from `event`, redacting field and enrichment values
/// through `engine`. Returns `false` when the event carries no log signal, in
/// which case `record` is left untouched and the caller must not emit it.
///
/// Records, in order: the log name; the severity number and text; the timestamp
/// the sink captured; the body; every log-routed field and enrichment; and the
/// call site.
///
/// The timestamp comes from [`EventView::timestamp`], not the host clock, so
/// every processor sharing one emission agrees on the instant and a frozen
/// [`tick::SimpleClock`] keeps the record deterministic.
pub(crate) fn populate_log_record(record: &mut impl LogRecord, event: &EventView<'_>, engine: &data_privacy::RedactionEngine) -> bool {
    // An event with no severity produces no log record at all, so the caller
    // skips it rather than emitting a record with no level.
    let Some(severity) = event.severity() else {
        return false;
    };

    // The log signal may carry its own name; fall back to the event name.
    let log_name = event.description().log().map_or_else(|| event.name(), LogDescription::name);
    record.set_event_name(log_name);

    record.set_severity_number(otel_severity_of(severity));
    record.set_severity_text(severity.as_str());
    record.set_timestamp(event.timestamp());
    if let Some(body) = event.body() {
        record.set_body(AnyValue::String(body.into()));
    }

    // Pulling a value is what invokes its redaction closure, so fields routed
    // away from logs are never redacted and never allocate.
    let _ = event.visit_fields(&mut |desc, get_value| {
        if let Some(log) = desc.log() {
            let value = any_value_of(get_value(engine));
            record.add_attribute(opentelemetry::Key::from_static_str(log.key()), value);
        }
        ControlFlow::Continue(())
    });
    let _ = event.visit_enrichments(&mut |desc, get_value| {
        if let Some(log) = desc.log() {
            let value = any_value_of(get_value(engine));
            record.add_attribute(opentelemetry::Key::from_static_str(log.key()), value);
        }
        ControlFlow::Continue(())
    });

    // File and line use stable OpenTelemetry code attributes. The emitting
    // crate is deliberately not exported: `code.namespace` is deprecated with
    // no standalone replacement, and its successor `code.function.name` needs a
    // fully qualified function name that the event model does not capture.
    // https://opentelemetry.io/docs/specs/semconv/registry/attributes/code/
    if let Some(file) = event.source_file() {
        record.add_attribute(opentelemetry::Key::from_static_str(CODE_FILE_PATH), AnyValue::String(file.into()));
    }
    if let Some(line) = event.source_line() {
        record.add_attribute(
            opentelemetry::Key::from_static_str(CODE_LINE_NUMBER),
            AnyValue::Int(i64::from(line)),
        );
    }
    true
}
