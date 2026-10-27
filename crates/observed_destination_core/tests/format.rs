// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public log formatting contracts.

#![cfg(test)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::time::{Duration, SystemTime};

use observed_destination_core::{LogFormatConfig, Rfc3339Nanos, print_log_record};
use opentelemetry::InstrumentationScope;
use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry_sdk::error::OTelSdkError;
use opentelemetry_sdk::logs::{SdkLogRecord, SdkLoggerProvider};

fn create_record_and_scope(configure: impl FnOnce(&mut SdkLogRecord)) -> (SdkLogRecord, InstrumentationScope) {
    let scope = InstrumentationScope::builder("test-lib").build();
    let provider = SdkLoggerProvider::builder().build();
    let logger = provider.logger("test-lib");
    let mut rec = logger.create_log_record();
    configure(&mut rec);
    (rec, scope)
}

fn format(record: &SdkLogRecord, scope: &InstrumentationScope, config: LogFormatConfig) -> String {
    let mut buf = [0; 1024];
    let mut remaining = buf.as_mut_slice();

    print_log_record(&mut remaining, record, scope, config).unwrap();

    let remaining = remaining.len();
    String::from_utf8(buf[..buf.len() - remaining].to_vec()).unwrap()
}

#[test]
fn format_body_string_inlines_attributes() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("hello {name}, {name}! {missing}".into()));
        r.add_attribute("name", "world");
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "hello world, world! {missing}\n");
}

#[test]
fn interpolation_does_not_replace_twice() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{path} {id}".into()));
        r.add_attribute("path", "/tmp/{id}");
        r.add_attribute("id", 42);
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "/tmp/{id} 42\n");
}

#[test]
fn interpolation_respects_escaped_braces() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{{name}} / {name}".into()));
        r.add_attribute("name", "Ada");
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "{{name}} / Ada\n");
}

#[test]
fn interpolation_preserves_text_after_unicode_prefix() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("é{id}X".into()));
        r.add_attribute("id", 42);
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "é42X\n");
}

#[test]
fn interpolation_preserves_text_after_unicode_key() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{café}X".into()));
        r.add_attribute("café", 42);
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "42X\n");
}

#[test]
fn interpolation_respects_escaped_braces_after_unicode() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("é{{name}} / {name}".into()));
        r.add_attribute("name", "Ada");
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "é{{name}} / Ada\n");
}

#[test]
fn interpolation_uses_first_duplicate_key() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{id}".into()));
        r.add_attribute("id", 1);
        r.add_attribute("id", 2);
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "1\n");
}

#[test]
fn interpolation_keeps_empty_placeholder_literal() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{}".into()));
        r.add_attribute("", "value");
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "{}\n");
}

#[test]
fn interpolation_keeps_unmatched_braces_literal() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{id} {unfinished {".into()));
        r.add_attribute("id", 42);
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "42 {unfinished {\n");
}

#[test]
fn interpolation_ends_placeholder_at_first_closing_brace() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_body(AnyValue::String("{a} {b}".into()));
        r.add_attribute("a} {b", "wrong");
        r.add_attribute("a", "A");
        r.add_attribute("b", "B");
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "A B\n");
}

#[test]
fn format_body_non_string_suppresses_attributes() {
    for (body, expected) in [
        (AnyValue::Int(42), "42"),
        (AnyValue::Boolean(true), "true"),
        (AnyValue::Double(1.5), "1.5"),
        (AnyValue::Bytes(Box::new(vec![0, 15, 255])), "000fff"),
        (
            AnyValue::ListAny(Box::new(vec![AnyValue::Int(42), AnyValue::Boolean(false)])),
            "[42, false]",
        ),
        (
            AnyValue::Map(Box::new([("answer".into(), AnyValue::Int(42))].into())),
            "{answer: 42}",
        ),
    ] {
        let (record, scope) = create_record_and_scope(|r| {
            r.set_body(body);
            r.add_attribute("extra", "should-not-appear");
        });

        let output = format(&record, &scope, LogFormatConfig::minimal());

        assert_eq!(output.trim_end(), expected);
    }
}

#[test]
fn no_format_body_prints_attributes_separately() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_severity_number(Severity::Warn);
        r.set_target("application");
        r.set_event_name("request.received");
        r.set_body(AnyValue::String("template {x}".into()));
        r.add_attribute("x", "val");
    });

    assert_eq!(
        format(&record, &scope, LogFormatConfig::default()),
        "WARN: application template {x} x=val [test-lib-?] <request.received>\n"
    );
}

#[test]
fn missing_body_prints_attributes_and_library_version() {
    let scope = InstrumentationScope::builder("test-lib").with_version("1.0").build();
    let (record, _) = create_record_and_scope(|r| r.add_attribute("x", "val"));

    assert_eq!(format(&record, &scope, LogFormatConfig::default()), " x=val [test-lib-1.0]\n");
}

#[test]
fn formatting_reports_write_failures_at_every_output_position() {
    for body in [
        AnyValue::String("request {id}".into()),
        AnyValue::String("{{id}}".into()),
        AnyValue::String("{missing}".into()),
        AnyValue::ListAny(Box::new(vec![
            AnyValue::Boolean(true),
            AnyValue::Int(42),
            AnyValue::Double(1.5),
            AnyValue::Map(Box::new([("bytes".into(), AnyValue::Bytes(Box::new(vec![0, 15, 255])))].into())),
        ])),
    ] {
        let (record, scope) = create_record_and_scope(|record| {
            record.set_timestamp(SystemTime::UNIX_EPOCH);
            record.set_severity_number(Severity::Warn);
            record.set_target("application");
            record.set_event_name("request.received");
            record.set_body(body);
            record.add_attribute("id", 42);
        });

        for config in [LogFormatConfig::default(), LogFormatConfig::minimal()] {
            let output = format(&record, &scope, config);

            // Fixed slices exercise failures within nested values and at the final newline.
            for capacity in 0..output.len() {
                let mut buffer = vec![0; capacity];

                let result = print_log_record(&mut buffer.as_mut_slice(), &record, &scope, config);

                assert!(matches!(result, Err(OTelSdkError::InternalFailure(_))));
            }
        }
    }
}

#[test]
fn timestamp_conversion_preserves_subseconds() {
    let timestamp = SystemTime::UNIX_EPOCH + Duration::from_millis(10_123);

    let timestamp = Rfc3339Nanos::from(timestamp).to_string();

    assert_eq!(timestamp, "1970-01-01T00:00:10.123000000Z");
}

#[test]
fn timestamp_conversion_preserves_pre_epoch_times() {
    let timestamp = SystemTime::UNIX_EPOCH - Duration::from_secs(1);

    let formatted = Rfc3339Nanos::from(timestamp).to_string();

    assert_eq!(formatted, "1969-12-31T23:59:59.000000000Z");
}

// Windows SystemTime cannot represent dates before 1601.
#[cfg(not(windows))]
#[test]
fn timestamp_conversion_saturates_below_minimum() {
    let timestamp = SystemTime::UNIX_EPOCH - Duration::from_secs(400_000_000_000);

    let formatted = Rfc3339Nanos::from(timestamp).to_string();

    assert_eq!(formatted, "0000-01-01T00:00:00.000000000Z");
}

// Windows SystemTime cannot represent dates before 1601.
#[cfg(not(windows))]
#[test]
fn timestamp_conversion_clamps_to_rfc3339_minimum() {
    let minimum = SystemTime::UNIX_EPOCH - Duration::from_hours(17_268_672);
    // These dates all convert to Jiff timestamps, including the negative year.
    for (timestamp, expected) in [
        (minimum - Duration::from_nanos(1), "0000-01-01T00:00:00.000000000Z"),
        (minimum, "0000-01-01T00:00:00.000000000Z"),
        (minimum + Duration::from_nanos(1), "0000-01-01T00:00:00.000000001Z"),
    ] {
        let formatted = Rfc3339Nanos::from(timestamp).to_string();

        assert_eq!(formatted, expected);
    }
}

#[test]
fn timestamp_conversion_saturates_above_maximum() {
    let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(400_000_000_000);

    let formatted = Rfc3339Nanos::from(timestamp).to_string();

    assert_eq!(formatted, "9999-12-30T22:00:00.999999999Z");
}

#[test]
fn timestamp_width_is_fixed_regardless_of_subsecond() {
    let render = |ts: SystemTime| {
        let (record, scope) = create_record_and_scope(|r| {
            r.set_timestamp(ts);
            r.set_body(AnyValue::String("body".into()));
        });

        format(&record, &scope, LogFormatConfig::minimal())
    };

    let zero_subsecond = render(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
    let with_subsecond = render(SystemTime::UNIX_EPOCH + Duration::new(10, 123_000_000));

    assert_eq!(zero_subsecond, "[1970-01-01T00:00:10.000000000Z] body\n");
    assert_eq!(with_subsecond, "[1970-01-01T00:00:10.123000000Z] body\n");
}

#[test]
fn observed_timestamp_is_used_when_timestamp_is_missing() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_observed_timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
        r.set_body(AnyValue::String("body".into()));
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "[1970-01-01T00:00:10.000000000Z] body\n");
}

#[test]
fn explicit_timestamp_takes_precedence_over_observed_timestamp() {
    let (record, scope) = create_record_and_scope(|r| {
        r.set_observed_timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
        r.set_timestamp(SystemTime::UNIX_EPOCH + Duration::new(10, 123_000_000));
        r.set_body(AnyValue::String("body".into()));
    });

    let output = format(&record, &scope, LogFormatConfig::minimal());

    assert_eq!(output, "[1970-01-01T00:00:10.123000000Z] body\n");
}
