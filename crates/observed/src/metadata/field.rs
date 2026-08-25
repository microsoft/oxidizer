// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field-level metadata and lazy field iteration.

use super::MetricDescription;

/// Per-field log routing entry.
#[derive(Debug, Clone, Copy)]
pub struct LogFieldEntry {
    key: &'static str,
}

impl LogFieldEntry {
    /// Creates a new log field entry.
    #[must_use]
    pub const fn new(key: &'static str) -> Self {
        Self { key }
    }

    /// Returns the log key for this field.
    #[must_use]
    #[inline]
    pub const fn key(&self) -> &'static str {
        self.key
    }
}

impl std::fmt::Display for LogFieldEntry {
    /// Renders the field's log key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key)
    }
}

/// Per-field metric routing entry.
///
/// If [`instrument_description`](Self::instrument_description) is `Some`, the field is the
/// measurement value recorded for that instrument and is **not** also used as
/// a dimension. Otherwise the field is a plain metric dimension keyed by
/// [`key`](Self::key).
#[derive(Debug, Clone, Copy)]
pub struct MetricFieldEntry {
    key: &'static str,
    instrument: Option<MetricDescription>,
}

impl MetricFieldEntry {
    /// Creates a dimension entry (no instrument).
    #[must_use]
    pub const fn dimension(key: &'static str) -> Self {
        Self { key, instrument: None }
    }

    /// Creates an instrument entry - the field carries this metric value.
    #[must_use]
    pub const fn instrument(key: &'static str, description: MetricDescription) -> Self {
        Self {
            key,
            instrument: Some(description),
        }
    }

    /// Returns the metric key for this field.
    ///
    /// For instrument-bearing fields this is the field's logical key (it is
    /// not used as a dimension).
    #[must_use]
    #[inline]
    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// Returns the metric instrument, if the field is a measurement source.
    #[must_use]
    #[inline]
    pub const fn instrument_description(&self) -> Option<&MetricDescription> {
        self.instrument.as_ref()
    }
}

/// Describes a single field on an event or enrichment entry.
///
/// Carries optional per-signal routing entries. A signal is enabled for a field
/// when its corresponding option is `Some`. Each entry carries the signal-specific
/// key (allowing different names in logs vs. metrics) and, for metrics, an
/// optional [`MetricDescription`] indicating the field is the metric *value*
/// rather than a dimension.
///
/// All keys are compile-time `'static` strings, so descriptors are `Copy` and
/// snapshotting consumers can retain them without allocating.
#[derive(Debug, Clone, Copy)]
pub struct FieldDescriptor {
    field_name: &'static str,
    log: Option<LogFieldEntry>,
    metric: Option<MetricFieldEntry>,
}

impl FieldDescriptor {
    /// Creates a descriptor with explicit per-signal entries.
    #[must_use]
    pub const fn new(field_name: &'static str, log: Option<LogFieldEntry>, metric: Option<MetricFieldEntry>) -> Self {
        Self { field_name, log, metric }
    }

    /// Creates a descriptor included in logs only.
    #[must_use]
    pub const fn log_only(key: &'static str) -> Self {
        Self {
            field_name: key,
            log: Some(LogFieldEntry::new(key)),
            metric: None,
        }
    }

    /// Returns the log routing entry for this field, if any.
    #[must_use]
    #[inline]
    pub const fn log(&self) -> Option<&LogFieldEntry> {
        self.log.as_ref()
    }

    /// Returns the metric routing entry for this field, if any.
    #[must_use]
    #[inline]
    pub const fn metric(&self) -> Option<&MetricFieldEntry> {
        self.metric.as_ref()
    }

    /// Returns the underlying field name, regardless of signal routing.
    #[must_use]
    #[inline]
    pub const fn field_name(&self) -> &'static str {
        self.field_name
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod field_tests {
    use super::*;
    use crate::metadata::InstrumentKind;

    #[test]
    fn metric_field_entry_instrument_accessors() {
        let desc = MetricDescription::new("http.server.duration", InstrumentKind::Histogram, "d", "ms");
        let entry = MetricFieldEntry::instrument("dur", desc);
        let instrument = entry.instrument_description().expect("instrument is set");
        assert_eq!(instrument.instrument_name(), "http.server.duration");
        assert_eq!(instrument.kind(), InstrumentKind::Histogram);
    }

    #[test]
    fn metric_field_entry_dimension_has_no_instrument() {
        let entry = MetricFieldEntry::dimension("region");
        assert!(entry.instrument_description().is_none());
    }

    #[test]
    fn log_field_entry_display_renders_the_key() {
        // `Display` is how the log key reaches diagnostics, so it must render
        // the bare key without any wrapper formatting.
        assert_eq!(LogFieldEntry::new("http.request.id").to_string(), "http.request.id");
    }

    #[test]
    fn descriptor_and_metric_entry_report_their_own_keys() {
        // The metric key may differ from the field name, so neither accessor
        // may answer with the other's string (or with a constant).
        let metric = MetricFieldEntry::dimension("region");
        assert_eq!(metric.key(), "region");

        let descriptor = FieldDescriptor::new("duration_ms", None, Some(metric));
        assert_eq!(descriptor.field_name(), "duration_ms");
        assert_eq!(descriptor.metric().expect("metric routing is set").key(), "region");

        // A log-only field carries no metric routing at all.
        assert!(FieldDescriptor::log_only("msg").metric().is_none());
    }
}
