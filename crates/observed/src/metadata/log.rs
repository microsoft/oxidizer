// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Log signal metadata.

use crate::severity::Severity;

/// Static description of an event's **log** signal.
///
/// Present on [`EventDescription::log`](crate::metadata::EventDescription::log)
/// when the event opts in to log emission (via `#[log(name = "…", severity = …)]`
/// on the event struct).
#[derive(Debug, Clone, Copy)]
pub struct LogDescription {
    /// The `OTel` log record event name (from `#[log(name = "...")]`, or the event name if omitted).
    name: &'static str,
    severity: Severity,
    body: Option<&'static str>,
}

impl LogDescription {
    /// Creates a new log description.
    #[must_use]
    pub const fn new(name: &'static str, severity: Severity, body: Option<&'static str>) -> Self {
        Self { name, severity, body }
    }

    /// Returns the `OTel` log record event name.
    ///
    /// This is the `name` from `#[log(name = "...")]`, or the event name if omitted.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the log severity.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// Returns the optional human-readable body.
    ///
    /// This is the message from `#[log(message = "...")]`. `None` when the event
    /// declares no message template.
    #[must_use]
    pub const fn body(&self) -> Option<&'static str> {
        self.body
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_description_name_roundtrips() {
        let desc = LogDescription::new("http.request", Severity::Info, Some("done"));
        assert_eq!(desc.name(), "http.request");
        assert_eq!(desc.body(), Some("done"));
    }
}
