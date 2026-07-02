// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end tests driving the rescaling provider through a real
//! `SdkMeterProvider` with an in-memory exporter, verifying that every
//! instrument kind produces a correctly rescaled sidecar alongside its original.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "unwrap, panic, and exact float comparisons keep tests concise and readable"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_rescaled::{RescaledMetrics, RescaledMetricsBuilder};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, Metric, MetricData, ResourceMetrics, ScopeMetrics, SumDataPoint};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

const SCOPE: &str = "test_scope";

struct Harness {
    outer: RescaledMetrics,
    sdk: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

impl Harness {
    fn new(configure: impl FnOnce(RescaledMetricsBuilder) -> RescaledMetrics) -> Self {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        let sdk = SdkMeterProvider::builder().with_reader(reader).build();
        let outer = configure(RescaledMetrics::builder(sdk.clone()));
        Self { outer, sdk, exporter }
    }

    /// Configures a single scope named [`SCOPE`].
    fn with_scope(configure: impl FnOnce(&mut opentelemetry_rescaled::ScopeConfigurator)) -> Self {
        Self::new(|builder| builder.scope(SCOPE, configure).build())
    }

    fn collect(&self) -> Vec<ResourceMetrics> {
        self.sdk.force_flush().unwrap();
        self.exporter.get_finished_metrics().unwrap()
    }
}

fn find<'a>(metrics: &'a [ResourceMetrics], name: &str) -> Option<&'a Metric> {
    metrics
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == name)
}

fn metric<'a>(metrics: &'a [ResourceMetrics], name: &str) -> &'a Metric {
    find(metrics, name).unwrap_or_else(|| panic!("metric '{name}' not found"))
}

fn sum_u64(metric: &Metric) -> u64 {
    match metric.data() {
        AggregatedMetrics::U64(MetricData::Sum(sum)) => sum.data_points().map(SumDataPoint::value).sum(),
        other => panic!("expected u64 sum, got {other:?}"),
    }
}

fn sum_f64(metric: &Metric) -> f64 {
    match metric.data() {
        AggregatedMetrics::F64(MetricData::Sum(sum)) => sum.data_points().map(SumDataPoint::value).sum(),
        other => panic!("expected f64 sum, got {other:?}"),
    }
}

fn sum_i64(metric: &Metric) -> i64 {
    match metric.data() {
        AggregatedMetrics::I64(MetricData::Sum(sum)) => sum.data_points().map(SumDataPoint::value).sum(),
        other => panic!("expected i64 sum, got {other:?}"),
    }
}

fn gauge_u64(metric: &Metric) -> u64 {
    match metric.data() {
        AggregatedMetrics::U64(MetricData::Gauge(gauge)) => gauge.data_points().next().unwrap().value(),
        other => panic!("expected u64 gauge, got {other:?}"),
    }
}

fn gauge_i64(metric: &Metric) -> i64 {
    match metric.data() {
        AggregatedMetrics::I64(MetricData::Gauge(gauge)) => gauge.data_points().next().unwrap().value(),
        other => panic!("expected i64 gauge, got {other:?}"),
    }
}

fn gauge_f64(metric: &Metric) -> f64 {
    match metric.data() {
        AggregatedMetrics::F64(MetricData::Gauge(gauge)) => gauge.data_points().next().unwrap().value(),
        other => panic!("expected f64 gauge, got {other:?}"),
    }
}

fn histogram_f64_sum(metric: &Metric) -> f64 {
    match metric.data() {
        AggregatedMetrics::F64(MetricData::Histogram(hist)) => hist.data_points().next().unwrap().sum(),
        other => panic!("expected f64 histogram, got {other:?}"),
    }
}

fn histogram_u64_sum(metric: &Metric) -> u64 {
    match metric.data() {
        AggregatedMetrics::U64(MetricData::Histogram(hist)) => hist.data_points().next().unwrap().sum(),
        other => panic!("expected u64 histogram, got {other:?}"),
    }
}

fn histogram_f64_bounds(metric: &Metric) -> Vec<f64> {
    match metric.data() {
        AggregatedMetrics::F64(MetricData::Histogram(hist)) => hist.data_points().next().unwrap().bounds().collect(),
        other => panic!("expected f64 histogram, got {other:?}"),
    }
}

// -------------------------------------------------------------------------
// Synchronous instruments
// -------------------------------------------------------------------------

#[test]
fn u64_counter_fans_out_with_metadata() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("bytes", "kilobytes", "kB", 0.001);
    });

    let counter = harness
        .outer
        .meter(SCOPE)
        .u64_counter("bytes")
        .with_description("bytes transferred")
        .with_unit("By")
        .build();
    counter.add(4000, &[]);

    let metrics = harness.collect();

    let original = metric(&metrics, "bytes");
    assert_eq!(sum_u64(original), 4000);
    assert_eq!(original.unit(), "By");
    assert_eq!(original.description(), "bytes transferred");

    let sidecar = metric(&metrics, "kilobytes");
    assert_eq!(sum_u64(sidecar), 4); // 4000 * 0.001
    assert_eq!(sidecar.unit(), "kB", "sidecar carries the configured unit");
    assert_eq!(
        sidecar.description(),
        "bytes transferred",
        "sidecar inherits the source description"
    );
}

#[test]
fn f64_counter_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("seconds", "millis", "ms", 1000.0);
    });

    let counter = harness.outer.meter(SCOPE).f64_counter("seconds").build();
    counter.add(1.5, &[]);
    counter.add(0.5, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_f64(metric(&metrics, "seconds")), 2.0);
    assert_eq!(sum_f64(metric(&metrics, "millis")), 2000.0);
}

#[test]
fn i64_up_down_counter_fans_out_with_negatives() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("delta", "delta.k", "k", 1000.0);
    });

    let updown = harness.outer.meter(SCOPE).i64_up_down_counter("delta").build();
    updown.add(5, &[]);
    updown.add(-2, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_i64(metric(&metrics, "delta")), 3);
    assert_eq!(sum_i64(metric(&metrics, "delta.k")), 3000);
}

#[test]
fn f64_up_down_counter_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("balance", "balance.milli", "m", 1000.0);
    });

    let updown = harness.outer.meter(SCOPE).f64_up_down_counter("balance").build();
    updown.add(2.5, &[]);
    updown.add(-1.0, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_f64(metric(&metrics, "balance")), 1.5);
    assert_eq!(sum_f64(metric(&metrics, "balance.milli")), 1500.0);
}

#[test]
fn u64_gauge_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("temp", "temp.milli", "m", 1000.0);
    });

    let gauge = harness.outer.meter(SCOPE).u64_gauge("temp").build();
    gauge.record(42, &[]);

    let metrics = harness.collect();
    assert_eq!(gauge_u64(metric(&metrics, "temp")), 42);
    assert_eq!(gauge_u64(metric(&metrics, "temp.milli")), 42000);
}

#[test]
fn i64_gauge_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("level", "level.k", "k", 1000.0);
    });

    let gauge = harness.outer.meter(SCOPE).i64_gauge("level").build();
    gauge.record(-7, &[]);

    let metrics = harness.collect();
    assert_eq!(gauge_i64(metric(&metrics, "level")), -7);
    assert_eq!(gauge_i64(metric(&metrics, "level.k")), -7000);
}

#[test]
fn f64_gauge_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("ratio", "ratio.pct", "%", 100.0);
    });

    let gauge = harness.outer.meter(SCOPE).f64_gauge("ratio").build();
    gauge.record(0.25, &[]);

    let metrics = harness.collect();
    assert_eq!(gauge_f64(metric(&metrics, "ratio")), 0.25);
    assert_eq!(gauge_f64(metric(&metrics, "ratio.pct")), 25.0);
}

// -------------------------------------------------------------------------
// Histograms — including bucket-boundary scaling
// -------------------------------------------------------------------------

#[test]
fn f64_histogram_scales_explicit_boundaries() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("dur", "dur.ms", "ms", 1000.0);
    });

    let histogram = harness
        .outer
        .meter(SCOPE)
        .f64_histogram("dur")
        .with_description("request duration")
        .with_boundaries(vec![1.0, 2.0, 3.0])
        .build();
    histogram.record(1.5, &[]);

    let metrics = harness.collect();

    assert_eq!(histogram_f64_sum(metric(&metrics, "dur")), 1.5);
    assert_eq!(metric(&metrics, "dur").description(), "request duration");
    assert_eq!(histogram_f64_bounds(metric(&metrics, "dur")), vec![1.0, 2.0, 3.0]);

    let sidecar = metric(&metrics, "dur.ms");
    assert_eq!(histogram_f64_sum(sidecar), 1500.0);
    assert_eq!(sidecar.unit(), "ms");
    assert_eq!(sidecar.description(), "request duration", "sidecar inherits the source description");
    assert_eq!(
        histogram_f64_bounds(sidecar),
        vec![1000.0, 2000.0, 3000.0],
        "sidecar boundaries are scaled by the same factor"
    );
}

#[test]
fn f64_histogram_without_boundaries_keeps_defaults() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("dur", "dur.ms", "ms", 1000.0);
    });

    let histogram = harness.outer.meter(SCOPE).f64_histogram("dur").build();
    histogram.record(1.5, &[]);

    let metrics = harness.collect();
    assert_eq!(histogram_f64_sum(metric(&metrics, "dur")), 1.5);
    assert_eq!(histogram_f64_sum(metric(&metrics, "dur.ms")), 1500.0);
    // Both keep the SDK default boundaries (nothing to scale).
    assert_eq!(
        histogram_f64_bounds(metric(&metrics, "dur")),
        histogram_f64_bounds(metric(&metrics, "dur.ms")),
    );
}

#[test]
fn u64_histogram_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("size", "size.kb", "kB", 0.001);
    });

    let histogram = harness
        .outer
        .meter(SCOPE)
        .u64_histogram("size")
        .with_boundaries(vec![1000.0, 2000.0])
        .build();
    histogram.record(3000, &[]);

    let metrics = harness.collect();
    assert_eq!(histogram_u64_sum(metric(&metrics, "size")), 3000);
    assert_eq!(histogram_u64_sum(metric(&metrics, "size.kb")), 3); // 3000 * 0.001
}

// -------------------------------------------------------------------------
// Observable instruments — dual registration
// -------------------------------------------------------------------------

#[test]
fn u64_observable_counter_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("obs", "obs.k", "k", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .u64_observable_counter("obs")
        .with_callback(|observer| observer.observe(100, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "obs")), 100);
    assert_eq!(sum_u64(metric(&metrics, "obs.k")), 100_000);
}

#[test]
fn f64_observable_gauge_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("g", "g.milli", "m", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .f64_observable_gauge("g")
        .with_callback(|observer| observer.observe(1.5, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(gauge_f64(metric(&metrics, "g")), 1.5);
    assert_eq!(gauge_f64(metric(&metrics, "g.milli")), 1500.0);
}

#[test]
fn i64_observable_gauge_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("g", "g.k", "k", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .i64_observable_gauge("g")
        .with_callback(|observer| observer.observe(-3, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(gauge_i64(metric(&metrics, "g")), -3);
    assert_eq!(gauge_i64(metric(&metrics, "g.k")), -3000);
}

#[test]
fn u64_observable_gauge_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("g", "g.k", "k", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .u64_observable_gauge("g")
        .with_callback(|observer| observer.observe(7, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(gauge_u64(metric(&metrics, "g")), 7);
    assert_eq!(gauge_u64(metric(&metrics, "g.k")), 7000);
}

#[test]
fn i64_observable_up_down_counter_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("obs", "obs.k", "k", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .i64_observable_up_down_counter("obs")
        .with_callback(|observer| observer.observe(-4, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(sum_i64(metric(&metrics, "obs")), -4);
    assert_eq!(sum_i64(metric(&metrics, "obs.k")), -4000);
}

#[test]
fn f64_observable_counter_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("obs", "obs.milli", "m", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .f64_observable_counter("obs")
        .with_callback(|observer| observer.observe(2.0, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(sum_f64(metric(&metrics, "obs")), 2.0);
    assert_eq!(sum_f64(metric(&metrics, "obs.milli")), 2000.0);
}

#[test]
fn f64_observable_up_down_counter_fans_out() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("obs", "obs.milli", "m", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .f64_observable_up_down_counter("obs")
        .with_callback(|observer| observer.observe(1.25, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(sum_f64(metric(&metrics, "obs")), 1.25);
    assert_eq!(sum_f64(metric(&metrics, "obs.milli")), 1250.0);
}

#[test]
fn observable_callback_runs_once_per_registered_instrument() {
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = Harness::with_scope(|scope| {
        scope.rescale("obs", "obs.k", "k", 1000.0);
    });

    let calls_in_cb = Arc::clone(&calls);
    let _instrument = harness
        .outer
        .meter(SCOPE)
        .u64_observable_counter("obs")
        .with_callback(move |observer| {
            calls_in_cb.fetch_add(1, Ordering::Relaxed);
            observer.observe(1, &[]);
        })
        .build();

    let _ = harness.collect();

    // Registered twice (original + sidecar), so the callback runs twice per collection.
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

// -------------------------------------------------------------------------
// Multiple sidecars, attributes, rounding
// -------------------------------------------------------------------------

#[test]
fn single_source_feeds_multiple_sidecars() {
    let harness = Harness::with_scope(|scope| {
        scope
            .rescale("dur", "dur.ms", "ms", 1000.0)
            .rescale("dur", "dur.us", "us", 1_000_000.0);
    });

    let counter = harness.outer.meter(SCOPE).f64_counter("dur").build();
    counter.add(2.0, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_f64(metric(&metrics, "dur")), 2.0);
    assert_eq!(sum_f64(metric(&metrics, "dur.ms")), 2000.0);
    assert_eq!(sum_f64(metric(&metrics, "dur.us")), 2_000_000.0);
}

#[test]
fn attributes_are_forwarded_to_sidecar() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("req", "req.k", "k", 1000.0);
    });

    let counter = harness.outer.meter(SCOPE).u64_counter("req").build();
    counter.add(1, &[KeyValue::new("route", "/health")]);

    let metrics = harness.collect();
    let sidecar = metric(&metrics, "req.k");
    let has_attr = match sidecar.data() {
        AggregatedMetrics::U64(MetricData::Sum(sum)) => sum.data_points().any(|dp| {
            dp.attributes()
                .any(|kv| kv.key.as_str() == "route" && kv.value.as_str() == "/health")
        }),
        other => panic!("expected u64 sum, got {other:?}"),
    };
    assert!(has_attr, "sidecar data point retains the recorded attributes");
}

#[test]
fn integer_rescale_rounds_and_saturates_end_to_end() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("halve", "halved", "x", 0.5).rescale("huge", "huge.big", "x", 1e30);
    });

    let meter = harness.outer.meter(SCOPE);
    let halve = meter.u64_counter("halve").build();
    halve.add(3, &[]); // 3 * 0.5 = 1.5 -> rounds to 2

    let huge = meter.u64_counter("huge").build();
    huge.add(u64::MAX, &[]); // saturates

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "halved")), 2);
    assert_eq!(sum_u64(metric(&metrics, "huge.big")), u64::MAX);
}

// -------------------------------------------------------------------------
// Pass-through behavior
// -------------------------------------------------------------------------

#[test]
fn unconfigured_instrument_in_configured_scope_has_no_sidecar() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("configured", "configured.k", "k", 1000.0);
    });

    let counter = harness.outer.meter(SCOPE).u64_counter("other").build();
    counter.add(5, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "other")), 5);
    assert!(
        find(&metrics, "configured.k").is_none(),
        "no sidecar for an instrument that was never created"
    );
    assert!(find(&metrics, "other.k").is_none());
}

#[test]
fn unconfigured_scope_passes_through() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("x", "x.k", "k", 1000.0);
    });

    let counter = harness.outer.meter("some_other_scope").u64_counter("x").build();
    counter.add(9, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "x")), 9);
    assert!(find(&metrics, "x.k").is_none(), "rules do not apply outside their configured scope");
}

#[test]
fn name_only_matching_applies_to_all_scopes_sharing_a_name() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("hits", "hits.k", "k", 1000.0);
    });

    // Two meters whose scopes share the name but differ in version.
    let scope_v1 = InstrumentationScope::builder(SCOPE).with_version("1.0").build();
    let scope_v2 = InstrumentationScope::builder(SCOPE).with_version("2.0").build();

    harness.outer.meter_with_scope(scope_v1).u64_counter("hits").build().add(1, &[]);
    harness.outer.meter_with_scope(scope_v2).u64_counter("hits").build().add(2, &[]);

    let metrics = harness.collect();
    // Both scopes emit a sidecar; summing across scopes gives (1 + 2) * 1000.
    let sidecar_total: u64 = metrics
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .filter(|m| m.name() == "hits.k")
        .map(sum_u64)
        .sum();
    assert_eq!(sidecar_total, 3000);
}

#[test]
fn multiple_scope_calls_accumulate_rules() {
    let harness = Harness::new(|builder| {
        builder
            .scope(SCOPE, |scope| {
                scope.rescale("a", "a.k", "k", 1000.0);
            })
            .scope(SCOPE, |scope| {
                scope.rescale("b", "b.k", "k", 1000.0);
            })
            .build()
    });

    let meter = harness.outer.meter(SCOPE);
    meter.u64_counter("a").build().add(1, &[]);
    meter.u64_counter("b").build().add(2, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "a.k")), 1000);
    assert_eq!(sum_u64(metric(&metrics, "b.k")), 2000);
}

#[test]
fn scope_without_rules_is_dropped_and_passes_through() {
    let harness = Harness::new(|builder| {
        builder
            .scope("empty_scope", |_scope| {
                // No rescale calls: this scope must not be wrapped.
            })
            .build()
    });

    let counter = harness.outer.meter("empty_scope").u64_counter("plain").build();
    counter.add(3, &[]);

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "plain")), 3);
}

// -------------------------------------------------------------------------
// No-rule instruments inside a configured scope (early-return branches)
// -------------------------------------------------------------------------

#[test]
fn unconfigured_histogram_in_configured_scope_has_no_sidecar() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("configured", "configured.ms", "ms", 1000.0);
    });

    let histogram = harness
        .outer
        .meter(SCOPE)
        .f64_histogram("other")
        .with_boundaries(vec![1.0, 2.0])
        .build();
    histogram.record(1.5, &[]);

    let metrics = harness.collect();
    assert_eq!(histogram_f64_sum(metric(&metrics, "other")), 1.5);
    assert!(
        find(&metrics, "other.ms").is_none(),
        "an unconfigured histogram gets no sidecar even inside a configured scope"
    );
}

#[test]
fn unconfigured_observable_in_configured_scope_has_no_sidecar() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("configured", "configured.k", "k", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .u64_observable_counter("other")
        .with_callback(|observer| observer.observe(5, &[]))
        .build();

    let metrics = harness.collect();
    assert_eq!(sum_u64(metric(&metrics, "other")), 5);
    assert!(
        find(&metrics, "other.k").is_none(),
        "an unconfigured observable gets no sidecar even inside a configured scope"
    );
}

// -------------------------------------------------------------------------
// Metadata inheritance for observables
// -------------------------------------------------------------------------

#[test]
fn observable_sidecar_inherits_description_and_carries_unit() {
    let harness = Harness::with_scope(|scope| {
        scope.rescale("obs", "obs.k", "k", 1000.0);
    });

    let _instrument = harness
        .outer
        .meter(SCOPE)
        .u64_observable_counter("obs")
        .with_description("observed count")
        .with_unit("things")
        .with_callback(|observer| observer.observe(3, &[]))
        .build();

    let metrics = harness.collect();

    let original = metric(&metrics, "obs");
    assert_eq!(sum_u64(original), 3);
    assert_eq!(original.description(), "observed count");
    assert_eq!(original.unit(), "things");

    let sidecar = metric(&metrics, "obs.k");
    assert_eq!(sum_u64(sidecar), 3000);
    assert_eq!(sidecar.description(), "observed count", "sidecar inherits the source description");
    assert_eq!(sidecar.unit(), "k", "sidecar carries its configured unit");
}

// -------------------------------------------------------------------------
// Debug formatting
// -------------------------------------------------------------------------

#[test]
fn debug_impls_render_scope_names() {
    let builder = RescaledMetrics::builder(SdkMeterProvider::builder().build()).scope(SCOPE, |scope| {
        scope.rescale("a", "a.k", "k", 1000.0);
    });
    let builder_debug = format!("{builder:?}");
    assert!(builder_debug.contains("RescaledMetricsBuilder"));
    assert!(builder_debug.contains(SCOPE));

    let provider = builder.build();
    let provider_debug = format!("{provider:?}");
    assert!(provider_debug.contains("RescaledMetrics"));
    assert!(provider_debug.contains(SCOPE));
}
