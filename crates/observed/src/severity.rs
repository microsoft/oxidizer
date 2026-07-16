// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Severity level for telemetry events.
///
/// Maps directly to OpenTelemetry severity levels, providing a simple enum
/// for event classification. Variants are ordered from least to most severe,
/// enabling severity-based filtering via comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Finest-grained informational events.
    Trace,
    /// Detailed debugging information.
    Debug,
    /// Informational events of general interest.
    Info,
    /// Warning events indicating potential issues.
    Warn,
    /// Error events indicating failures.
    Error,
    /// Critical errors that may cause system shutdown.
    Fatal,
}

impl Severity {
    /// Returns the severity as a static string label (e.g. `"WARN"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }
}

impl From<Severity> for opentelemetry::logs::Severity {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Trace => Self::Trace,
            Severity::Debug => Self::Debug,
            Severity::Info => Self::Info,
            Severity::Warn => Self::Warn,
            Severity::Error => Self::Error,
            Severity::Fatal => Self::Fatal,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_as_str_is_uppercase() {
        let cases = [
            (Severity::Trace, "TRACE"),
            (Severity::Debug, "DEBUG"),
            (Severity::Info, "INFO"),
            (Severity::Warn, "WARN"),
            (Severity::Error, "ERROR"),
            (Severity::Fatal, "FATAL"),
        ];
        for (severity, expected) in cases {
            assert_eq!(severity.as_str(), expected);
        }
    }

    #[test]
    fn severity_converts_to_otel() {
        let otel: opentelemetry::logs::Severity = Severity::Warn.into();
        assert_eq!(otel, opentelemetry::logs::Severity::Warn);
    }
}
