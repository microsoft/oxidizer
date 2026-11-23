// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local metric instrument cache used by [`MetricEventProcessor`](crate::MetricEventProcessor).
//!
//! Instruments are created lazily on first use per thread and cached for
//! subsequent recordings. Uses thread-local storage so the hot path is lock-free.

use std::any::type_name;
use std::borrow::Cow;
use std::cell::RefCell;

use observed::metadata::{InstrumentKind, MetricDescription};
use opentelemetry::metrics::Meter;
use opentelemetry_sdk::metrics::InstrumentKind as OtelInstrumentKind;
use rustc_hash::FxHashMap;
use thread_local::ThreadLocal;

/// A cached `OTel` metric instrument.
enum Instrument {
    // TODO: why they limited only to f64? We probably need to support all numeric types
    Histogram(opentelemetry::metrics::Histogram<f64>),
    Gauge(opentelemetry::metrics::Gauge<f64>),
    UpDownCounter(opentelemetry::metrics::UpDownCounter<f64>),
    Counter(opentelemetry::metrics::Counter<f64>),
}

impl Instrument {
    fn record(&self, value: f64, attributes: &[opentelemetry::KeyValue]) {
        match self {
            Self::Histogram(h) => h.record(value, attributes),
            Self::Gauge(g) => g.record(value, attributes),
            Self::UpDownCounter(c) => c.add(value, attributes),
            Self::Counter(c) => c.add(value, attributes),
        }
    }
}

/// The identifying properties of an instrument.
///
/// Copied from `opentelemetry_sdk` 0.31.0 (`metrics::instrument::InstrumentId`),
/// which is not exported from the crate. Keying the cache the same way the SDK
/// does internally means two `MetricDescription`s the SDK would treat as the
/// same instrument also share a cache entry.
///
/// Source: <https://github.com/open-telemetry/opentelemetry-rust/blob/v0.31.0/opentelemetry-sdk/src/metrics/instrument.rs#L331-L358>
#[derive(Debug, PartialEq, Eq, Hash)]
struct InstrumentId {
    /// The human-readable identifier of the instrument.
    name: Cow<'static, str>,
    /// Describes the purpose of the data.
    description: Cow<'static, str>,
    /// Defines the functional group of the instrument.
    kind: OtelInstrumentKind,
    /// The unit of measurement recorded.
    unit: Cow<'static, str>,
    /// Number is the underlying data type of the instrument.
    number: Cow<'static, str>,
}

impl InstrumentId {
    /// Instrument names are considered case-insensitive ASCII.
    ///
    /// Standardize the instrument name to always be lowercase so it can be
    /// compared via hash.
    ///
    /// See [naming syntax] for full requirements.
    ///
    /// [naming syntax]: https://github.com/open-telemetry/opentelemetry-specification/blob/v1.21.0/specification/metrics/api.md#instrument-name-syntax
    fn normalize(&mut self) {
        if self.name.chars().any(|c| c.is_ascii_uppercase()) {
            self.name = self.name.to_ascii_lowercase().into();
        }
    }

    /// Builds an `InstrumentId` from a `MetricDescription`, normalizing the
    /// name.
    /// `number` is always `"f64"` because [`Instrument`] only builds `f64` instruments.
    fn from_metric(metric: &MetricDescription) -> Self {
        let kind = match metric.kind() {
            InstrumentKind::Histogram => OtelInstrumentKind::Histogram,
            InstrumentKind::Gauge => OtelInstrumentKind::Gauge,
            InstrumentKind::UpDownCounter => OtelInstrumentKind::UpDownCounter,
            // `InstrumentKind` is `#[non_exhaustive]`; `Counter` and any
            // future variant both fall back to `Counter`.
            InstrumentKind::Counter | _ => OtelInstrumentKind::Counter,
        };
        let mut id = Self {
            name: Cow::Borrowed(metric.instrument_name()),
            description: Cow::Borrowed(metric.description()),
            kind,
            unit: Cow::Borrowed(metric.unit()),
            number: Cow::Borrowed("f64"),
        };
        id.normalize();
        id
    }
}

/// Thread-local cache of `OTel` metric instruments, keyed by [`InstrumentId`].
///
/// Each thread maintains its own `FxHashMap` of cached instruments.
/// No locks or atomics on the hot path.
pub(crate) struct InstrumentCache {
    meter: Meter,
    cache: ThreadLocal<RefCell<FxHashMap<InstrumentId, Instrument>>>,
}

impl InstrumentCache {
    /// Creates a new empty instrument slot backed by the given meter.
    pub(crate) fn new(meter: Meter) -> Self {
        Self {
            meter,
            cache: ThreadLocal::new(),
        }
    }

    /// Resolves the instrument for the given metric description (creating and
    /// caching it on first use per thread), then records the data point.
    pub(crate) fn record(&self, metric: &MetricDescription, value: f64, attributes: &[opentelemetry::KeyValue]) {
        let cell = self.cache.get_or(|| RefCell::new(FxHashMap::default()));
        let mut cache = cell.borrow_mut();

        let instrument = cache
            .entry(InstrumentId::from_metric(metric))
            .or_insert_with(|| self.create(metric));

        instrument.record(value, attributes);
    }

    fn create(&self, metric: &MetricDescription) -> Instrument {
        // `InstrumentKind` is `#[non_exhaustive]`; `Counter` and any future
        // variant both fall back to `Counter`.
        match metric.kind() {
            InstrumentKind::Histogram => Instrument::Histogram(
                self.meter
                    .f64_histogram(metric.instrument_name())
                    .with_description(metric.description())
                    .with_unit(metric.unit())
                    .build(),
            ),
            InstrumentKind::Gauge => Instrument::Gauge(
                self.meter
                    .f64_gauge(metric.instrument_name())
                    .with_description(metric.description())
                    .with_unit(metric.unit())
                    .build(),
            ),
            InstrumentKind::UpDownCounter => Instrument::UpDownCounter(
                self.meter
                    .f64_up_down_counter(metric.instrument_name())
                    .with_description(metric.description())
                    .with_unit(metric.unit())
                    .build(),
            ),
            InstrumentKind::Counter | _ => Instrument::Counter(
                self.meter
                    .f64_counter(metric.instrument_name())
                    .with_description(metric.description())
                    .with_unit(metric.unit())
                    .build(),
            ),
        }
    }
}

impl std::fmt::Debug for InstrumentCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use opentelemetry::metrics::MeterProvider;
    use opentelemetry::metrics::noop::NoopMeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    use super::*;

    fn md(name: &'static str, kind: InstrumentKind) -> MetricDescription {
        MetricDescription::new(name, kind, "", "")
    }

    fn hash_of<T: Hash>(v: &T) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    fn record_samples(description: &MetricDescription, values: &[f64]) -> Vec<ResourceMetrics> {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        let cache = InstrumentCache::new(provider.meter("cache"));

        for &value in values {
            cache.record(description, value, &[]);
        }

        provider.force_flush().unwrap();
        exporter.get_finished_metrics().unwrap()
    }

    #[test]
    fn name_is_lowercased_when_mixed_case() {
        let id = InstrumentId::from_metric(&md("Foo.Bar", InstrumentKind::Gauge));
        assert_eq!(id.name, "foo.bar");
    }

    #[test]
    fn already_lowercase_name_is_borrowed_not_allocated() {
        let id = InstrumentId::from_metric(&md("foo.bar", InstrumentKind::Histogram));
        assert!(matches!(id.name, Cow::Borrowed(_)));
    }

    #[test]
    fn case_insensitive_names_are_equal_and_hash_equal() {
        let a = InstrumentId::from_metric(&md("Foo.Bar", InstrumentKind::Histogram));
        let b = InstrumentId::from_metric(&md("foo.bar", InstrumentKind::Histogram));
        let c = InstrumentId::from_metric(&md("FOO.BAR", InstrumentKind::Histogram));
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_eq!(hash_of(&a), hash_of(&c));
    }

    #[test]
    fn formats_cache_debug() {
        let cache = InstrumentCache::new(NoopMeterProvider::new().meter("cache"));

        assert!(format!("{cache:?}").contains("InstrumentCache"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel MeterProviderBuilder eagerly calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn histogram_records_all_samples() {
        let description = MetricDescription::new("test.request.size", InstrumentKind::Histogram, "Request payload size", "By");

        let batches = record_samples(&description, &[3.0, 7.0]);

        let metric = batches
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .next()
            .unwrap();
        assert_eq!(metric.name(), description.instrument_name());
        assert_eq!(metric.description(), "Request payload size");
        assert_eq!(metric.unit(), "By");

        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
            panic!("expected histogram");
        };
        let point = histogram.data_points().next().unwrap();
        // A histogram must retain both measurements, not only the latest value.
        assert_eq!(point.count(), 2);
        assert!((point.sum() - 10.0).abs() < 1e-9);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel MeterProviderBuilder eagerly calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn counter_accumulates_increments() {
        let batches = record_samples(&md("test.requests", InstrumentKind::Counter), &[3.0, 7.0]);

        let metric = batches
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .next()
            .unwrap();
        let AggregatedMetrics::F64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected counter");
        };
        // Counter descriptions must select a monotonic accumulator, not a gauge or up-down counter.
        assert!(sum.is_monotonic());
        assert!((sum.data_points().next().unwrap().value() - 10.0).abs() < 1e-9);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel MeterProviderBuilder eagerly calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn gauge_records_latest_value() {
        let batches = record_samples(&md("test.queue_depth", InstrumentKind::Gauge), &[3.0, 7.0]);

        let metric = batches
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .next()
            .unwrap();
        let AggregatedMetrics::F64(MetricData::Gauge(gauge)) = metric.data() else {
            panic!("expected gauge");
        };
        // A gauge replaces its previous measurement instead of accumulating it.
        assert!((gauge.data_points().next().unwrap().value() - 7.0).abs() < 1e-9);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel MeterProviderBuilder eagerly calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn up_down_counter_accumulates_increments_and_decrements() {
        // Both signs must accumulate on the same instrument rather than replace its value.
        let batches = record_samples(&md("test.active_requests", InstrumentKind::UpDownCounter), &[3.0, -1.0]);

        let metric = batches
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .next()
            .unwrap();
        let AggregatedMetrics::F64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected up-down counter");
        };
        // A gauge would retain -1, and a monotonic counter cannot represent a decrement.
        assert!(!sum.is_monotonic());
        assert!((sum.data_points().next().unwrap().value() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn same_name_different_kind_is_not_equal() {
        let h = InstrumentId::from_metric(&md("foo.bar", InstrumentKind::Histogram));
        let g = InstrumentId::from_metric(&md("foo.bar", InstrumentKind::Gauge));
        let c = InstrumentId::from_metric(&md("foo.bar", InstrumentKind::UpDownCounter));
        assert_ne!(h, g);
        assert_ne!(h, c);
        assert_ne!(g, c);
    }

    #[test]
    fn emit_kind_maps_to_otel_kind() {
        let h = InstrumentId::from_metric(&md("x", InstrumentKind::Histogram));
        let g = InstrumentId::from_metric(&md("x", InstrumentKind::Gauge));
        let c = InstrumentId::from_metric(&md("x", InstrumentKind::UpDownCounter));
        let ct = InstrumentId::from_metric(&md("x", InstrumentKind::Counter));
        assert_eq!(h.kind, OtelInstrumentKind::Histogram);
        assert_eq!(g.kind, OtelInstrumentKind::Gauge);
        assert_eq!(c.kind, OtelInstrumentKind::UpDownCounter);
        assert_eq!(ct.kind, OtelInstrumentKind::Counter);
    }

    #[test]
    fn description_and_unit_are_part_of_the_key() {
        let base = MetricDescription::new("foo.bar", InstrumentKind::Histogram, "", "");
        let with_desc = MetricDescription::new("foo.bar", InstrumentKind::Histogram, "Some metric", "");
        let with_unit = MetricDescription::new("foo.bar", InstrumentKind::Histogram, "", "ms");

        assert_ne!(InstrumentId::from_metric(&base), InstrumentId::from_metric(&with_desc));
        assert_ne!(InstrumentId::from_metric(&base), InstrumentId::from_metric(&with_unit));
    }

    #[test]
    fn description_and_unit_are_propagated_to_id() {
        let id = InstrumentId::from_metric(&MetricDescription::new("foo.bar", InstrumentKind::Histogram, "Some metric", "ms"));
        assert_eq!(id.description, "Some metric");
        assert_eq!(id.unit, "ms");
    }
}
