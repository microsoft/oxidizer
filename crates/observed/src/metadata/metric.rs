// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Metric signal metadata.

/// Metric instrument kind for per-field metric routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InstrumentKind {
    /// A monotonically increasing sum (only non-negative increments).
    Counter,
    /// A sum that can go up or down.
    UpDownCounter,
    /// A point-in-time value.
    Gauge,
    /// A distribution of measured values.
    Histogram,
}

impl std::fmt::Display for InstrumentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Counter => write!(f, "Counter"),
            Self::UpDownCounter => write!(f, "UpDownCounter"),
            Self::Gauge => write!(f, "Gauge"),
            Self::Histogram => write!(f, "Histogram"),
        }
    }
}

/// Numeric representation of a metric measurement after widening without loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetricValueKind {
    /// Unsigned integer measurements, widened to `u64`.
    U64,
    /// Signed integer measurements, widened to `i64`.
    I64,
    /// Floating-point measurements, widened to `f64`.
    F64,
}

/// Describes a metric instrument derived from event metadata.
///
/// Two kinds of metric descriptions exist:
/// - **Field-level**: embedded in [`FieldDescriptor::metric`](crate::metadata::FieldDescriptor::metric).
///   The field's value is the measurement recorded for this instrument.
/// - **Event-level**: returned by [`EventDescription::metric`](crate::metadata::EventDescription::metric).
///   Records `1.0` per emission (count semantics, no associated field value).
#[derive(Debug, Clone, Copy)]
pub struct MetricDescription {
    /// The `OTel` instrument name (e.g. `"http.server.request.duration"`).
    instrument_name: &'static str,
    /// The instrument kind.
    kind: InstrumentKind,
    /// Human-readable description of what the instrument measures. Empty if unset.
    description: &'static str,
    /// Unit of measurement (e.g. `"ms"`, `"By"`, `"{request}"`). Empty if unset.
    ///
    /// Should follow [UCUM] when describing physical units.
    ///
    /// [UCUM]: https://ucum.org/ucum
    unit: &'static str,
    value_kind: MetricValueKind,
}

impl MetricDescription {
    #[must_use]
    pub(crate) const fn new(
        instrument_name: &'static str,
        kind: InstrumentKind,
        description: &'static str,
        unit: &'static str,
        value_kind: MetricValueKind,
    ) -> Self {
        Self {
            instrument_name,
            kind,
            description,
            unit,
            value_kind,
        }
    }

    /// Numeric representation of the instrument's measurement.
    ///
    /// The event macro derives this from the field type. Fieldless counters
    /// use [`MetricValueKind::U64`] and record one per emission.
    #[must_use]
    pub const fn value_kind(&self) -> MetricValueKind {
        self.value_kind
    }

    /// Returns the `OTel` instrument name.
    #[must_use]
    pub const fn instrument_name(&self) -> &'static str {
        self.instrument_name
    }

    /// Returns the instrument kind.
    #[must_use]
    pub const fn kind(&self) -> InstrumentKind {
        self.kind
    }

    /// Returns the instrument description, or `""` if unset.
    #[must_use]
    pub const fn description(&self) -> &'static str {
        self.description
    }

    /// Returns the instrument unit, or `""` if unset.
    #[must_use]
    pub const fn unit(&self) -> &'static str {
        self.unit
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;

    static_assertions::assert_impl_all!(MetricValueKind: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(MetricDescription: UnwindSafe, RefUnwindSafe);

    #[test]
    fn runtime_description_preserves_the_selected_numeric_representation() {
        for kind in [MetricValueKind::U64, MetricValueKind::I64, MetricValueKind::F64] {
            let metric = MetricDescription::new("value", InstrumentKind::Gauge, "", "", kind);
            assert_eq!(metric.value_kind(), kind);
        }
    }

    #[test]
    fn metadata_preserves_numeric_representation_and_explicit_text() {
        let defaults = MetricDescription::new("count", InstrumentKind::Counter, "", "", MetricValueKind::U64);
        assert_eq!(
            (
                defaults.instrument_name(),
                defaults.kind(),
                defaults.description(),
                defaults.unit(),
                defaults.value_kind()
            ),
            ("count", InstrumentKind::Counter, "", "", MetricValueKind::U64),
        );
        let explicit = MetricDescription::new("size", InstrumentKind::Histogram, "size", "By", MetricValueKind::F64);
        assert_eq!(
            (
                explicit.instrument_name(),
                explicit.kind(),
                explicit.description(),
                explicit.unit(),
                explicit.value_kind()
            ),
            ("size", InstrumentKind::Histogram, "size", "By", MetricValueKind::F64),
        );
    }

    #[test]
    fn instrument_kind_display_is_nonempty_and_unique() {
        use std::collections::HashSet;

        let variants = [
            InstrumentKind::Counter,
            InstrumentKind::UpDownCounter,
            InstrumentKind::Gauge,
            InstrumentKind::Histogram,
        ];
        let mut seen = HashSet::new();
        for kind in variants {
            let rendered = kind.to_string();
            assert!(!rendered.is_empty(), "{kind:?} renders an empty string");
            assert!(seen.insert(rendered.clone()), "duplicate Display output {rendered:?} for {kind:?}");
        }
    }

    #[test]
    fn metric_description_reports_description_and_unit_verbatim() {
        // Both reach the exported instrument as-is, and they must not be
        // transposed with one another or with the instrument name.
        let desc = MetricDescription::new(
            "http.server.duration",
            InstrumentKind::Histogram,
            "server duration",
            "ms",
            MetricValueKind::F64,
        );

        assert_eq!(desc.description(), "server duration");
        assert_eq!(desc.unit(), "ms");
    }
}
