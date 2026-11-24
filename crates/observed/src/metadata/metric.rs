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
    /// Bucket boundaries of a histogram. Empty if unset.
    boundaries: &'static [f64],
}

impl MetricDescription {
    /// Creates a new metric description. `description` and `unit` may be `""`
    /// to leave them unset on the underlying instrument.
    #[must_use]
    pub const fn new(instrument_name: &'static str, kind: InstrumentKind, description: &'static str, unit: &'static str) -> Self {
        Self {
            instrument_name,
            kind,
            description,
            unit,
            boundaries: &[],
        }
    }

    /// Sets the bucket boundaries of a histogram.
    ///
    /// An exporter that creates an OpenTelemetry histogram for the instrument passes the
    /// boundaries to it. OpenTelemetry requires finite values in strictly increasing order.
    #[must_use]
    pub const fn with_boundaries(mut self, boundaries: &'static [f64]) -> Self {
        self.boundaries = boundaries;
        self
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

    /// Returns the bucket boundaries of a histogram, or an empty slice if unset.
    #[must_use]
    pub const fn boundaries(&self) -> &'static [f64] {
        self.boundaries
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

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
        let desc = MetricDescription::new("http.server.duration", InstrumentKind::Histogram, "server duration", "ms");

        assert_eq!(desc.description(), "server duration");
        assert_eq!(desc.unit(), "ms");
    }

    #[test]
    fn metric_description_keeps_the_boundaries_it_is_given() {
        let plain = MetricDescription::new("http.server.duration", InstrumentKind::Histogram, "", "s");
        let bucketed = plain.with_boundaries(&[0.5, 1.0]);

        assert_eq!((plain.boundaries(), bucketed.boundaries()), (&[][..], &[0.5, 1.0][..]));
    }
}
