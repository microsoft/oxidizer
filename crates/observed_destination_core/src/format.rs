// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared log record formatting used by stdout and file destinations.

use std::fmt;
use std::io::Write;
use std::time::SystemTime;

use jiff::Timestamp;
use opentelemetry::logs::AnyValue;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};

/// Configuration for log formatting.
///
/// Controls which parts of a log record are included in the output.
///
/// # Examples
///
/// ```
/// use observed_destination_core::LogFormatConfig;
///
/// // Default includes everything
/// let config = LogFormatConfig::default();
///
/// // Minimal format: timestamp, severity, body only (with placeholders expanded)
/// let config = LogFormatConfig::minimal();
/// ```
#[derive(Debug, Copy, Clone)]
#[expect(clippy::struct_excessive_bools, reason = "config struct with boolean flags")]
pub struct LogFormatConfig {
    /// Include target (e.g., `observed_destination_stdout::tests`).
    pub include_target: bool,
    /// Include instrumentation library info (e.g., `[name-version]`).
    pub include_library: bool,
    /// Include event name (e.g., `<database_tracker.run.started>`).
    pub include_event_name: bool,
    /// Format body by expanding attribute placeholders (e.g., `request received, id={request.id}`
    /// becomes `request received, id=10`). When `true`, attributes are inlined into the message
    /// body template and not printed separately.
    pub format_body: bool,
}

impl Default for LogFormatConfig {
    fn default() -> Self {
        Self {
            include_target: true,
            include_library: true,
            include_event_name: true,
            format_body: false,
        }
    }
}

impl LogFormatConfig {
    /// Creates a minimal format configuration (only timestamp, severity, and
    /// message body with placeholders expanded).
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            include_target: false,
            include_library: false,
            include_event_name: false,
            format_body: true,
        }
    }
}

/// Display wrapper for [`AnyValue`] that avoids Rust debug formatting
/// (e.g. `String(Static("…"))`) and instead renders human-readable values.
struct DisplayAnyValue<'a>(&'a AnyValue);

impl fmt::Display for DisplayAnyValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            AnyValue::String(s) => write!(f, "{s}"),
            AnyValue::Boolean(b) => write!(f, "{b}"),
            AnyValue::Int(i) => write!(f, "{i}"),
            AnyValue::Double(d) => write!(f, "{d}"),
            AnyValue::Bytes(bytes) => {
                write!(f, "0x")?;
                for byte in bytes.iter() {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
            AnyValue::ListAny(l) => {
                write!(f, "[")?;
                for v in l.iter() {
                    write!(f, "{}, ", DisplayAnyValue(v))?;
                }
                write!(f, "]")
            }
            AnyValue::Map(m) => {
                write!(f, "{{")?;
                for (k, v) in m.iter() {
                    write!(f, "{k}: {}, ", DisplayAnyValue(v))?;
                }
                write!(f, "}}")
            }
            // Closure to skip coverage for a non-exhaustive match arm.
            _ => ({
                #[cfg_attr(coverage_nightly, coverage(off))]
                || write!(f, "{:?}", self.0)
            })(),
        }
    }
}

#[expect(clippy::needless_pass_by_value, reason = "better ergonomics")]
fn io_err(e: std::io::Error) -> OTelSdkError {
    OTelSdkError::InternalFailure(e.to_string())
}

/// Renders a [`SystemTime`] as a fixed-width, nanosecond-precision RFC 3339 UTC
/// timestamp (e.g. `1970-01-01T00:00:10.000000000Z`).
///
/// The `{:.9}` precision is what keeps every log line the same width: jiff's
/// default `Display` for [`Timestamp`] omits trailing zero fractional digits,
/// which produces inconsistent, occasionally shorter timestamps.
#[derive(Debug)]
pub struct Rfc3339Nanos(pub SystemTime);

impl fmt::Display for Rfc3339Nanos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = Timestamp::try_from(self.0).unwrap_or(Timestamp::MAX);
        write!(f, "{ts:.9}")
    }
}

pub(crate) fn print_resource(w: &mut (impl Write + ?Sized), resource: &Resource) -> OTelSdkResult {
    (|| {
        write!(w, "<Resource: ")?;
        if let Some(schema_url) = resource.schema_url() {
            write!(w, "{schema_url}")?;
        }
        for (k, v) in resource {
            write!(w, " {k}={}", v.as_str())?;
        }
        write!(w, ">")
    })()
    .map_err(io_err)
}

/// Use when exporting SDK log records to a text destination, passing its writer
/// and formatting configuration. Appends a newline; the caller controls buffering
/// and flushing.
pub fn print_log_record(
    w: &mut (impl Write + ?Sized),
    record: &opentelemetry_sdk::logs::SdkLogRecord,
    scope: &opentelemetry::InstrumentationScope,
    config: LogFormatConfig,
) -> OTelSdkResult {
    (|| {
        // Prefer timestamp; fall back to observed_timestamp (set by the tracing bridge).
        if let Some(ts) = record.timestamp().or_else(|| record.observed_timestamp()) {
            write!(w, "[{}] ", Rfc3339Nanos(ts))?;
        }

        if let Some(severity) = record.severity_number() {
            write!(w, "{}: ", severity.name())?;
        }

        if config.include_target
            && let Some(target) = record.target()
        {
            write!(w, "{target} ")?;
        }

        let mut formatted_body_printed = false;

        if let Some(body) = record.body() {
            if config.format_body {
                if let AnyValue::String(template) = body {
                    let mut result = template.to_string();

                    for (key, value) in record.attributes_iter() {
                        let placeholder = format!("{{{key}}}");
                        result = result.replace(&placeholder, &DisplayAnyValue(value).to_string());
                    }

                    write!(w, "{result}")?;
                } else {
                    write!(w, "{}", DisplayAnyValue(body))?;
                }
                formatted_body_printed = true;
            } else {
                write!(w, "{}", DisplayAnyValue(body))?;
            }
        }

        if !formatted_body_printed {
            for (k, v) in record.attributes_iter() {
                write!(w, " {k}={}", DisplayAnyValue(v))?;
            }
        }

        if config.include_library {
            write!(w, " [{}-{}]", scope.name(), scope.version().unwrap_or("?"))?;
        }

        if config.include_event_name
            && let Some(event_name) = record.event_name()
        {
            write!(w, " <{event_name}>")?;
        }

        writeln!(w)
    })()
    .map_err(io_err)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use opentelemetry::InstrumentationScope;
    use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _, Severity};
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use super::*;

    fn create_record_and_scope(
        configure: impl FnOnce(&mut opentelemetry_sdk::logs::SdkLogRecord),
    ) -> (opentelemetry_sdk::logs::SdkLogRecord, InstrumentationScope) {
        let scope = InstrumentationScope::builder("test-lib").build();
        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("test-lib");
        let mut rec = logger.create_log_record();
        configure(&mut rec);
        (rec, scope)
    }

    fn format(record: &opentelemetry_sdk::logs::SdkLogRecord, scope: &InstrumentationScope, config: LogFormatConfig) -> String {
        let mut buf = Vec::new();

        print_log_record(&mut buf, record, scope, config).unwrap();

        String::from_utf8(buf).unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn format_body_string_inlines_attributes() {
        let (record, scope) = create_record_and_scope(|r| {
            r.set_body(AnyValue::String("hello {name}, {name}! {missing}".into()));
            r.add_attribute("name", "world");
        });

        let output = format(&record, &scope, LogFormatConfig::minimal());

        insta::assert_snapshot!(output, @"hello world, world! {missing}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn format_body_non_string_suppresses_attributes() {
        for (body, expected) in [
            (AnyValue::Int(42), "42"),
            (AnyValue::Boolean(true), "true"),
            (AnyValue::Double(1.5), "1.5"),
            (AnyValue::Bytes(Box::new(vec![0, 15, 255])), "0x000fff"),
            (
                AnyValue::ListAny(Box::new(vec![AnyValue::Int(42), AnyValue::Boolean(false)])),
                "[42, false, ]",
            ),
            (
                AnyValue::Map(Box::new([("answer".into(), AnyValue::Int(42))].into())),
                "{answer: 42, }",
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
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn no_format_body_prints_attributes_separately() {
        let (record, scope) = create_record_and_scope(|r| {
            r.set_severity_number(Severity::Warn);
            r.set_target("application");
            r.set_event_name("request.received");
            r.set_body(AnyValue::String("template {x}".into()));
            r.add_attribute("x", "val");
        });

        insta::assert_snapshot!(
            format(&record, &scope, LogFormatConfig::default()),
            @"WARN: application template {x} x=val [test-lib-?] <request.received>"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn missing_body_prints_attributes_and_library_version() {
        let scope = InstrumentationScope::builder("test-lib").with_version("1.0").build();
        let (record, _) = create_record_and_scope(|r| r.add_attribute("x", "val"));

        assert_eq!(format(&record, &scope, LogFormatConfig::default()), " x=val [test-lib-1.0]\n");
    }

    #[test]
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn formatting_reports_write_failures_at_every_output_position() {
        for body in [
            AnyValue::String("request {id}".into()),
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
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
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
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn observed_timestamp_is_used_when_timestamp_is_missing() {
        let (record, scope) = create_record_and_scope(|r| {
            r.set_observed_timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
            r.set_body(AnyValue::String("body".into()));
        });

        let output = format(&record, &scope, LogFormatConfig::minimal());

        assert_eq!(output, "[1970-01-01T00:00:10.000000000Z] body\n");
    }

    #[test]
    #[cfg_attr(miri, ignore = "OTel SDK logger construction calls current_exe, unsupported in Miri isolation")]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn explicit_timestamp_takes_precedence_over_observed_timestamp() {
        let (record, scope) = create_record_and_scope(|r| {
            r.set_observed_timestamp(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
            r.set_timestamp(SystemTime::UNIX_EPOCH + Duration::new(10, 123_000_000));
            r.set_body(AnyValue::String("body".into()));
        });

        let output = format(&record, &scope, LogFormatConfig::minimal());

        assert_eq!(output, "[1970-01-01T00:00:10.123000000Z] body\n");
    }
}
