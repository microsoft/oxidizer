// Copyright (c) Microsoft Corporation.

//! Full providers remain responsible for metering and lifecycle independently of the cache.

use performables::arc::Arc;
use performables::sync::mutex::Mutex;
use std::error::Error;
use std::io;
use std::sync::Arc as StdArc;
use std::time::Duration;

use data_privacy::{DataClass, RedactionEngine};
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, emit, event};
use observed_destination_core::{DestinationResult, FlushableMetricsProvider};
use observed_otel::MetricEventProcessor;
use opentelemetry::metrics::{InstrumentProvider, Meter, MeterProvider};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ScopeMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tick::SimpleClock;

const PRIVATE: DataClass = DataClass::new("test", "private");

#[derive(Debug)]
struct TrackingProvider {
    inner: SdkMeterProvider,
    calls: Arc<Mutex<Vec<&'static str>>>,
    fail_flush: bool,
}

impl MeterProvider for TrackingProvider {
    fn meter_with_scope(&self, scope: InstrumentationScope) -> Meter {
        self.calls.lock().push("meter");
        self.inner.meter_with_scope(scope)
    }
}

impl FlushableMetricsProvider for TrackingProvider {
    fn force_flush(&self) -> DestinationResult<()> {
        self.calls.lock().push("flush");
        if self.fail_flush {
            Err(io::Error::other("test flush failure").into())
        } else {
            self.inner.force_flush().map_err(Into::into)
        }
    }
}

impl Drop for TrackingProvider {
    fn drop(&mut self) {
        self.calls.lock().push("drop");
    }
}

#[derive(Debug)]
struct DropOrderProvider(Arc<Mutex<Vec<&'static str>>>);

impl MeterProvider for DropOrderProvider {
    fn meter_with_scope(&self, _scope: InstrumentationScope) -> Meter {
        self.0.lock().push("meter");
        Meter::new(StdArc::new(DropOrderMeter(Arc::clone(&self.0))))
    }
}

impl FlushableMetricsProvider for DropOrderProvider {
    fn force_flush(&self) -> DestinationResult<()> {
        self.0.lock().push("flush");
        Ok(())
    }
}

impl Drop for DropOrderProvider {
    fn drop(&mut self) {
        self.0.lock().push("provider-drop");
    }
}

#[derive(Debug)]
struct DropOrderMeter(Arc<Mutex<Vec<&'static str>>>);

impl InstrumentProvider for DropOrderMeter {}

impl Drop for DropOrderMeter {
    fn drop(&mut self) {
        self.0.lock().push("cache-drop");
    }
}

#[test]
fn cached_meter_is_released_before_provider() {
    let calls = Arc::new(Mutex::<_>::new(Vec::new()));
    let processor = MetricEventProcessor::new(
        DropOrderProvider(Arc::clone(&calls)),
        RedactionEngine::builder().build(),
        None,
    );
    drop(processor);
    assert_eq!(
        *calls.lock(),
        ["meter", "flush", "cache-drop", "provider-drop"]
    );
}

#[event("external.request")]
#[counter(name = "external.requests")]
#[histogram(duration_ms, name = "external.duration", unit = "ms")]
struct Request {
    #[unredacted]
    duration_ms: f64,
    #[unredacted]
    #[dimension(metric = "outcome")]
    outcome: &'static str,
    #[data_class(PRIVATE)]
    #[dimension(metric = "classified")]
    classified: &'static str,
}

#[derive(Enrichment)]
struct Context {
    #[unredacted]
    #[dimension(metric = "region")]
    region: &'static str,
    #[unredacted]
    request_id: &'static str,
}

#[test]
#[cfg_attr(miri, ignore = "SDK resource detection uses current_exe")]
fn full_provider_records_metadata_and_redacted_dimensions() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    let scope = InstrumentationScope::builder("external.host")
        .with_version("1.2.3")
        .with_schema_url("https://example.com/schema")
        .with_attributes([KeyValue::new("scope.label", "host")])
        .build();
    let calls = Arc::new(Mutex::<_>::new(Vec::new()));
    let sink = observed_otel::builder()
        .processor(MetricEventProcessor::new(
            TrackingProvider {
                inner: provider,
                calls: Arc::clone(&calls),
                fail_flush: false,
            },
            RedactionEngine::builder().build(),
            Some(scope.clone()),
        ))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    (|| {
        emit!(
            sink,
            Request {
                duration_ms: 4.5,
                outcome: "success",
                classified: "must-not-escape",
            }
        );
    })
    .enrich(
        &sink,
        Context {
            region: "test",
            request_id: "log-only",
        },
    )();
    sink.flush().unwrap();

    let batches = exporter.get_finished_metrics().unwrap();
    let scoped = batches[0].scope_metrics().next().unwrap();
    assert_eq!(scoped.scope(), &scope);
    let metrics: Vec<_> = scoped.metrics().collect();
    assert_eq!(metrics.len(), 2);
    for metric in metrics {
        let attributes = match metric.data() {
            AggregatedMetrics::F64(MetricData::Sum(sum)) => {
                assert_eq!(metric.name(), "external.requests");
                let point = sum.data_points().next().unwrap();
                assert!((point.value() - 1.0).abs() < f64::EPSILON);
                point.attributes().cloned().collect::<Vec<_>>()
            }
            AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                assert_eq!(metric.name(), "external.duration");
                assert_eq!(metric.unit(), "ms");
                let point = histogram.data_points().next().unwrap();
                assert_eq!(point.count(), 1);
                assert!((point.sum() - 4.5).abs() < f64::EPSILON);
                point.attributes().cloned().collect::<Vec<_>>()
            }
            other => panic!("unexpected metric data: {other:?}"),
        };
        assert!(attributes.contains(&KeyValue::new("outcome", "success")));
        assert!(attributes.contains(&KeyValue::new("region", "test")));
        assert!(
            attributes
                .iter()
                .any(|attribute| attribute.key.as_str() == "classified")
        );
        assert!(!attributes.iter().any(|attribute| {
            attribute.value.to_string().contains("must-not-escape")
                || attribute.key.as_str() == "request_id"
                || attribute.key.as_str() == "duration_ms"
        }));
    }
    drop(sink);
    assert_eq!(*calls.lock(), ["meter", "flush", "flush", "drop"]);
}

#[test]
#[cfg_attr(miri, ignore = "SDK resource detection uses current_exe")]
fn provider_is_owned_even_when_no_instruments_are_cached() {
    let calls = Arc::new(Mutex::<_>::new(Vec::new()));
    let sink = observed_otel::builder()
        .processor(MetricEventProcessor::new(
            TrackingProvider {
                inner: SdkMeterProvider::builder().build(),
                calls: Arc::clone(&calls),
                fail_flush: false,
            },
            RedactionEngine::builder().build(),
            None,
        ))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    assert_eq!(*calls.lock(), ["meter"]);

    let last_sink = sink.clone();
    sink.flush().unwrap();
    sink.flush().unwrap();
    drop(sink);
    assert_eq!(*calls.lock(), ["meter", "flush", "flush"]);
    drop(last_sink);
    assert_eq!(*calls.lock(), ["meter", "flush", "flush", "flush", "drop"]);
}

#[test]
#[cfg_attr(miri, ignore = "SDK resource detection uses current_exe")]
fn provider_flush_errors_are_surfaced_and_do_not_skip_drop() {
    let calls = Arc::new(Mutex::<_>::new(Vec::new()));
    let sink = observed_otel::builder()
        .processor(MetricEventProcessor::new(
            TrackingProvider {
                inner: SdkMeterProvider::builder().build(),
                calls: Arc::clone(&calls),
                fail_flush: true,
            },
            RedactionEngine::builder().build(),
            None,
        ))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    let error = sink.flush().unwrap_err();
    let failure = &error.failures()[0];
    assert_eq!(failure.processor(), "otel-metrics");
    assert_eq!(
        failure
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::Other
    );
    drop(sink);
    assert_eq!(*calls.lock(), ["meter", "flush", "flush", "drop"]);
}

#[test]
#[cfg_attr(miri, ignore = "SDK resource detection uses current_exe")]
fn sink_drop_exports_pending_metrics_and_keeps_shared_provider_usable() {
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone())
        .with_interval(Duration::from_secs(3600))
        .build();
    let provider = SdkMeterProvider::builder().with_reader(reader).build();
    let sink = observed_otel::builder()
        .processor(MetricEventProcessor::new(
            provider.clone(),
            RedactionEngine::builder().build(),
            None,
        ))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    emit!(
        sink,
        Request {
            duration_ms: 4.5,
            outcome: "success",
            classified: "private",
        }
    );
    assert!(exporter.get_finished_metrics().unwrap().is_empty());

    drop(sink);

    let batches = exporter.get_finished_metrics().unwrap();
    assert_eq!(batches.len(), 1);
    let duration = batches[0]
        .scope_metrics()
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == "external.duration")
        .unwrap();
    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = duration.data() else {
        panic!("expected the pending histogram");
    };
    let point = histogram.data_points().next().unwrap();
    assert_eq!(point.count(), 1);
    assert!((point.sum() - 4.5).abs() < f64::EPSILON);

    provider
        .meter("shared")
        .f64_counter("after.drop")
        .build()
        .add(2.0, &[]);
    provider.force_flush().unwrap();
    let batches = exporter.get_finished_metrics().unwrap();
    let after = batches
        .last()
        .unwrap()
        .scope_metrics()
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == "after.drop")
        .unwrap();
    let AggregatedMetrics::F64(MetricData::Sum(sum)) = after.data() else {
        panic!("expected a counter recorded after sink drop");
    };
    assert!((sum.data_points().next().unwrap().value() - 2.0).abs() < f64::EPSILON);
    provider.shutdown().unwrap();
}
