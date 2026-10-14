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

/// A numeric metric measurement, separate from general telemetry field values.
///
/// Generated access preserves signedness and integer precision. This value
/// carries no dimensions, redaction, or destination-specific aggregation.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum MetricValue {
    /// An unsigned integer increment or reading.
    U64(u64),
    /// A signed integer delta or reading.
    I64(i64),
    /// A floating-point observation.
    F64(f64),
}

impl MetricValue {
    /// The numeric representation of this measurement.
    #[must_use]
    #[inline]
    pub const fn kind(self) -> MetricValueKind {
        match self {
            Self::U64(_) => MetricValueKind::U64,
            Self::I64(_) => MetricValueKind::I64,
            Self::F64(_) => MetricValueKind::F64,
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
    value_kind: Option<MetricValueKind>,
}

impl MetricDescription {
    /// Configures an instrument with optional description, unit, and numeric representation.
    ///
    /// The name and instrument kind are required. Description and unit default
    /// to empty strings; numeric representation defaults to unspecified.
    ///
    /// ```
    /// use observed::metadata::{InstrumentKind, MetricDescription, MetricValueKind};
    ///
    /// const BYTES: MetricDescription = MetricDescription::builder("bytes", InstrumentKind::Counter)
    ///     .description("Bytes sent")
    ///     .unit("By")
    ///     .value_kind(MetricValueKind::U64)
    ///     .build();
    ///
    /// assert_eq!(BYTES.unit(), "By");
    /// ```
    #[must_use]
    pub const fn builder(instrument_name: &'static str, kind: InstrumentKind) -> MetricDescriptionBuilder {
        MetricDescriptionBuilder {
            metric: Self {
                instrument_name,
                kind,
                description: "",
                unit: "",
                value_kind: None,
            },
        }
    }

    /// Creates a new metric description. `description` and `unit` may be `""`
    /// to leave them unset on the underlying instrument.
    #[must_use]
    pub const fn new(instrument_name: &'static str, kind: InstrumentKind, description: &'static str, unit: &'static str) -> Self {
        Self::builder(instrument_name, kind).description(description).unit(unit).build()
    }

    /// Numeric representation, or `None` for legacy or unspecified metadata.
    #[must_use]
    pub const fn value_kind(&self) -> Option<MetricValueKind> {
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

/// Builds constant metric metadata with required name and instrument kind.
///
/// Start with [`MetricDescription::builder`]. Optional metadata is configured
/// here rather than through mutating methods on [`MetricDescription`].
#[derive(Debug, Clone, Copy)]
pub struct MetricDescriptionBuilder {
    metric: MetricDescription,
}

impl MetricDescriptionBuilder {
    /// Sets the instrument's human-readable description.
    #[must_use]
    pub const fn description(mut self, description: &'static str) -> Self {
        self.metric.description = description;
        self
    }

    /// Sets the unit of measurement, such as `"By"` or `"ms"`.
    #[must_use]
    pub const fn unit(mut self, unit: &'static str) -> Self {
        self.metric.unit = unit;
        self
    }

    /// Declares the representation returned by direct measurement access.
    ///
    /// Generated [`Event::METRICS`](crate::Event::METRICS) includes this metadata
    /// so backends can reject unsupported numeric types during startup.
    #[must_use]
    pub const fn value_kind(mut self, kind: MetricValueKind) -> Self {
        self.metric.value_kind = Some(kind);
        self
    }

    /// Finishes the instrument description without allocation.
    #[must_use]
    pub const fn build(self) -> MetricDescription {
        self.metric
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::any::type_name;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;

    static_assertions::assert_impl_all!(MetricValueKind: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(MetricValue: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(MetricDescriptionBuilder: UnwindSafe, RefUnwindSafe);

    #[test]
    fn runtime_description_preserves_the_selected_numeric_representation() {
        for kind in [MetricValueKind::U64, MetricValueKind::I64, MetricValueKind::F64] {
            let metric = MetricDescription::builder("value", InstrumentKind::Gauge).value_kind(kind).build();
            assert_eq!(metric.value_kind(), Some(kind));
        }
    }

    #[test]
    fn builder_defaults_and_explicit_metadata_match_the_legacy_constructor() {
        let defaults = MetricDescription::builder("count", InstrumentKind::Counter).build();
        assert_eq!(
            (
                defaults.instrument_name(),
                defaults.kind(),
                defaults.description(),
                defaults.unit(),
                defaults.value_kind()
            ),
            ("count", InstrumentKind::Counter, "", "", None),
        );
        let builder = MetricDescription::builder("size", InstrumentKind::Histogram)
            .description("size")
            .unit("By");
        let explicit = builder.build();
        let legacy = MetricDescription::new("size", InstrumentKind::Histogram, "size", "By");
        assert_eq!(
            (
                explicit.instrument_name(),
                explicit.kind(),
                explicit.description(),
                explicit.unit(),
                explicit.value_kind()
            ),
            (
                legacy.instrument_name(),
                legacy.kind(),
                legacy.description(),
                legacy.unit(),
                legacy.value_kind()
            ),
        );
        assert!(format!("{builder:?}").contains(type_name::<MetricDescriptionBuilder>().rsplit("::").next().unwrap()));
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
        let desc = MetricDescription::new("http.server.duration", InstrumentKind::Histogram, "server duration", "ms");

        assert_eq!(desc.description(), "server duration");
        assert_eq!(desc.unit(), "ms");
    }
}
