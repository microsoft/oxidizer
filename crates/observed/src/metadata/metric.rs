// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Metric signal metadata.

/// Metric instrument kind for per-field metric routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// The instrument kind (histogram, gauge, or counter).
    kind: InstrumentKind,
    /// Human-readable description of what the instrument measures. Empty if unset.
    description: &'static str,
    /// Unit of measurement (e.g. `"ms"`, `"By"`, `"{request}"`). Empty if unset.
    ///
    /// Should follow [UCUM] when describing physical units.
    ///
    /// [UCUM]: https://ucum.org/ucum
    unit: &'static str,
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
        }
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
    use super::*;

    #[test]
    fn instrument_kind_display() {
        assert_eq!(InstrumentKind::Counter.to_string(), "Counter");
        assert_eq!(InstrumentKind::UpDownCounter.to_string(), "UpDownCounter");
        assert_eq!(InstrumentKind::Gauge.to_string(), "Gauge");
        assert_eq!(InstrumentKind::Histogram.to_string(), "Histogram");
    }
}
