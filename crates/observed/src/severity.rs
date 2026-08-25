// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Severity levels for telemetry events.

/// Severity level for telemetry events.
///
/// Levels mirror the OpenTelemetry severity model. Variants are ordered from
/// least to most severe, enabling severity-based filtering via comparison
/// operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_labels_are_nonempty_uppercase_and_unique() {
        use std::collections::HashSet;

        let variants = [
            Severity::Trace,
            Severity::Debug,
            Severity::Info,
            Severity::Warn,
            Severity::Error,
            Severity::Fatal,
        ];
        let mut seen = HashSet::new();
        for severity in variants {
            let label = severity.as_str();
            assert!(!label.is_empty(), "{severity:?} has an empty label");
            assert!(
                label.chars().all(|c| c.is_ascii_uppercase()),
                "{severity:?} label {label:?} is not all uppercase"
            );
            assert!(seen.insert(label), "duplicate label {label:?}");
        }
    }

    #[test]
    fn severity_orders_from_least_to_most_severe() {
        assert!(Severity::Trace < Severity::Debug);
        assert!(Severity::Debug < Severity::Info);
        assert!(Severity::Info < Severity::Warn);
        assert!(Severity::Warn < Severity::Error);
        assert!(Severity::Error < Severity::Fatal);
    }
}
