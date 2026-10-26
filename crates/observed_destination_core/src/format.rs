// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared log record formatting used by stdout and file destinations.

use std::fmt;
use std::io::Write;
use std::time::SystemTime;

use jiff::Timestamp;
use observed_utils::format_any_value;
use opentelemetry::logs::AnyValue;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::SdkLogRecord;

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
    /// body template and not printed separately. If the body is absent, attributes are printed separately.
    pub format_body: bool,
}

impl Default for LogFormatConfig {
    /// Creates a configuration that includes all fields and leaves message placeholders unchanged.
    ///
    /// # Examples
    ///
    /// The three `include_*` fields are `true`, so the output includes the target,
    /// library information, and event name.
    /// With `format_body` set to `false`, `{request.id}` stays in the message.
    /// The attribute appears separately as `request.id=10`.
    ///
    /// ```
    /// use observed_destination_core::{LogFormatConfig, print_log_record};
    /// use opentelemetry::InstrumentationScope;
    /// use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
    /// use opentelemetry_sdk::logs::SdkLoggerProvider;
    ///
    /// let scope = InstrumentationScope::builder("example").with_version("1.0").build();
    /// let provider = SdkLoggerProvider::builder().build();
    /// let logger = provider.logger_with_scope(scope.clone());
    /// let mut record = logger.create_log_record();
    /// record.set_target("application");
    /// record.set_event_name("request.received");
    /// record.set_body("request received, id={request.id}".into());
    /// record.add_attribute("request.id", 10);
    ///
    /// let config = LogFormatConfig::default();
    /// let mut output = Vec::new();
    /// print_log_record(&mut output, &record, &scope, config)?;
    ///
    /// assert_eq!(
    ///     output,
    ///     b"application request received, id={request.id} request.id=10 [example-1.0] <request.received>\n",
    /// );
    /// # Ok::<(), opentelemetry_sdk::error::OTelSdkError>(())
    /// ```
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
    /// Creates a minimal format configuration.
    ///
    /// Only timestamp, severity, and message body with placeholders expanded.
    ///
    /// # Examples
    ///
    /// With `format_body` set to `true`, `{request.id}` becomes `10`.
    /// Attributes are not printed separately.
    /// The three `include_*` fields are `false`, so the output omits the target,
    /// library information, and event name set below.
    ///
    /// ```
    /// use observed_destination_core::{LogFormatConfig, print_log_record};
    /// use opentelemetry::InstrumentationScope;
    /// use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
    /// use opentelemetry_sdk::logs::SdkLoggerProvider;
    ///
    /// let scope = InstrumentationScope::builder("example").build();
    /// let provider = SdkLoggerProvider::builder().build();
    /// let logger = provider.logger_with_scope(scope.clone());
    /// let mut record = logger.create_log_record();
    /// record.set_target("application");
    /// record.set_event_name("request.received");
    /// record.set_body("request received, id={request.id}".into());
    /// record.add_attribute("request.id", 10);
    ///
    /// let config = LogFormatConfig::minimal();
    /// let mut output = Vec::new();
    /// print_log_record(&mut output, &record, &scope, config)?;
    ///
    /// assert_eq!(output, b"request received, id=10\n");
    /// # Ok::<(), opentelemetry_sdk::error::OTelSdkError>(())
    /// ```
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
///
/// Times outside Jiff's range use the nearest supported boundary.
///
/// # Examples
///
/// ```
/// use std::io::Write as _;
/// use std::time::{Duration, SystemTime};
///
/// use observed_destination_core::Rfc3339Nanos;
///
/// let timestamp = Rfc3339Nanos::from(SystemTime::UNIX_EPOCH + Duration::from_secs(10));
/// let mut output = Vec::new();
/// writeln!(output, "[{timestamp}] WARN: dropped 3 events")?;
///
/// assert_eq!(
///     output,
///     b"[1970-01-01T00:00:10.000000000Z] WARN: dropped 3 events\n"
/// );
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Rfc3339Nanos(SystemTime);

impl From<SystemTime> for Rfc3339Nanos {
    fn from(ts: SystemTime) -> Self {
        Self(ts)
    }
}

impl fmt::Display for Rfc3339Nanos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = Timestamp::try_from(self.0).unwrap_or_else(|_| timestamp_boundary(self.0));
        write!(f, "{ts:.9}")
    }
}

fn timestamp_boundary(system_time: SystemTime) -> Timestamp {
    if system_time.duration_since(SystemTime::UNIX_EPOCH).is_err() {
        Timestamp::MIN
    } else {
        Timestamp::MAX
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

/// Writes one formatted log record as a text line to `w`.
///
/// Uses `config` to select the fields and format the message body.
/// Appends a newline. The caller controls buffering and flushing.
///
/// The output order is `[timestamp] SEVERITY: target body key=value [scope-version] <event>`.
/// The timestamp uses [`Rfc3339Nanos`]. If the record timestamp is absent, the observed timestamp is used.
/// If both timestamps are absent, no timestamp is printed.
/// Absent severity, target, body, and event fields are skipped.
/// The configuration controls whether the target, scope, and event are included.
/// A missing scope version uses `?`.
///
/// If `config.format_body` is `true` and a body is present, attributes are not printed separately.
/// In this case, attribute values replace matching placeholders in string bodies.
/// If `config.format_body` is `false` or the body is absent, attributes are printed as `key=value` pairs after the body.
///
/// # Examples
///
/// ```
/// use observed_destination_core::{LogFormatConfig, print_log_record};
/// use opentelemetry::InstrumentationScope;
/// use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
/// use opentelemetry_sdk::logs::SdkLoggerProvider;
///
/// let scope = InstrumentationScope::builder("example").build();
/// let provider = SdkLoggerProvider::builder().build();
/// let logger = provider.logger_with_scope(scope.clone());
/// let mut record = logger.create_log_record();
/// record.set_body("request received, id={request.id}".into());
/// record.add_attribute("request.id", 10);
///
/// let mut output = Vec::new();
/// print_log_record(&mut output, &record, &scope, LogFormatConfig::minimal())?;
///
/// assert_eq!(output, b"request received, id=10\n");
/// # Ok::<(), opentelemetry_sdk::error::OTelSdkError>(())
/// ```
///
/// # Errors
///
/// Returns an [`OTelSdkError::InternalFailure`] with the reason for the underlying failure,
/// if writing to the provided writer fails.
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
                    write_interpolated(w, template.as_str(), record)?;
                } else {
                    write!(w, "{}", format_any_value(body))?;
                }
                formatted_body_printed = true;
            } else {
                write!(w, "{}", format_any_value(body))?;
            }
        }

        if !formatted_body_printed {
            for (k, v) in record.attributes_iter() {
                write!(w, " {k}={}", format_any_value(v))?;
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

fn write_interpolated(w: &mut (impl Write + ?Sized), template: &str, record: &SdkLogRecord) -> std::io::Result<()> {
    let mut remaining = template;

    while let Some(offset) = remaining.find('{') {
        w.write_all(&remaining.as_bytes()[..offset])?;
        remaining = &remaining[offset..];

        if remaining.starts_with("{{") {
            w.write_all(&remaining.as_bytes()[..2])?;
            remaining = &remaining[2..];
            continue;
        }

        let Some((placeholder, remainder)) = remaining[1..].split_once('}') else {
            break;
        };

        if let Some((_, value)) = record
            .attributes_iter()
            .find(|(key, _)| !placeholder.is_empty() && key.as_str() == placeholder)
        {
            write!(w, "{}", format_any_value(value))?;
        } else {
            w.write_all(&remaining.as_bytes()[..placeholder.len() + 2])?;
        }
        remaining = remainder;
    }

    w.write_all(remaining.as_bytes())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn timestamp_boundary_before_epoch_is_minimum() {
        // Test the fallback choice without a date that Windows cannot represent.
        let timestamp = SystemTime::UNIX_EPOCH - Duration::from_secs(1);

        let boundary = timestamp_boundary(timestamp);

        assert_eq!(boundary, Timestamp::MIN);
    }

    #[test]
    fn timestamp_boundary_after_epoch_is_maximum() {
        let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1);

        let boundary = timestamp_boundary(timestamp);

        assert_eq!(boundary, Timestamp::MAX);
    }
}
