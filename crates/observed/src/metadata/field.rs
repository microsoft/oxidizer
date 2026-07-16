// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field-level metadata and lazy field iteration.

use super::{InstrumentKind, MetricDescription};

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
    pub const fn key(&self) -> &'static str {
        self.key
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
    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// Returns the metric instrument, if the field is a measurement source.
    #[must_use]
    pub const fn instrument_description(&self) -> Option<&MetricDescription> {
        self.instrument.as_ref()
    }

    /// Returns the instrument name when this entry is a measurement source.
    #[must_use]
    pub fn instrument_name(&self) -> Option<&'static str> {
        self.instrument.as_ref().map(MetricDescription::instrument_name)
    }

    /// Returns the instrument kind when this entry is a measurement source.
    #[must_use]
    pub fn kind(&self) -> Option<InstrumentKind> {
        self.instrument.as_ref().map(MetricDescription::kind)
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
    pub const fn log(&self) -> Option<&LogFieldEntry> {
        self.log.as_ref()
    }

    /// Returns the metric routing entry for this field, if any.
    #[must_use]
    pub const fn metric(&self) -> Option<&MetricFieldEntry> {
        self.metric.as_ref()
    }

    /// Returns the underlying field name, regardless of signal routing.
    #[must_use]
    pub const fn field_name(&self) -> &'static str {
        self.field_name
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod field_tests {
    use super::*;

    #[test]
    fn metric_field_entry_instrument_accessors() {
        let desc = MetricDescription::new("http.server.duration", InstrumentKind::Histogram, "d", "ms");
        let entry = MetricFieldEntry::instrument("dur", desc);
        assert_eq!(entry.instrument_name(), Some("http.server.duration"));
        assert_eq!(entry.kind(), Some(InstrumentKind::Histogram));
    }

    #[test]
    fn metric_field_entry_dimension_has_no_instrument() {
        let entry = MetricFieldEntry::dimension("region");
        assert_eq!(entry.instrument_name(), None);
        assert_eq!(entry.kind(), None);
    }
}
