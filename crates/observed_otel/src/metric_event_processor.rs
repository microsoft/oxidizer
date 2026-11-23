// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::ControlFlow;

use observed::FlushError;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed_destination_core::FlushableMetricsProvider;

use crate::instrument_cache::InstrumentCache;

const DEFAULT_METER_NAME: &str = "observed_meter";

/// An [`EventProcessor`] that dispatches events as metric recordings through `OTel`.
///
/// Instruments are created lazily on first use per thread and cached in an
/// internal lock-free `thread_local` cache.
///
/// Retains a full [`FlushableMetricsProvider`] independently of its instrument
/// cache. [`flush`](Self::flush) delegates to the provider's `force_flush`, and
/// [`Drop`] flushes best-effort before releasing the provider and cached
/// instruments. It does not shut down a provider retained by another consumer.
/// Terminal cleanup follows the provider's own ownership and drop semantics.
///
/// Drain producers and call [`EventProcessor::flush`] while exporter/runtime
/// resources are still available to surface errors. Drop-based flushing is a
/// fallback; it cannot report failures or cover measurements recorded afterward.
pub struct MetricEventProcessor {
    instruments: InstrumentCache,
    // Rust drops fields in declaration order; retain the provider through cache cleanup.
    provider: Box<dyn FlushableMetricsProvider>,
    redaction_engine: data_privacy::RedactionEngine,
}

impl MetricEventProcessor {
    /// Creates a metric processor that retains and flushes the given provider.
    ///
    /// Works with [`SdkMeterProvider`](opentelemetry_sdk::metrics::SdkMeterProvider)
    /// and other providers implementing [`FlushableMetricsProvider`].
    /// All meters and instruments are obtained from this provider; releasing the
    /// cache is not a request to shut down the pipeline.
    #[must_use]
    pub fn new(
        provider: impl FlushableMetricsProvider + 'static,
        redaction_engine: data_privacy::RedactionEngine,
        scope_override: Option<opentelemetry::InstrumentationScope>,
    ) -> Self {
        let meter = match scope_override {
            Some(s) => provider.meter_with_scope(s),
            None => provider.meter(DEFAULT_METER_NAME),
        };

        Self {
            instruments: InstrumentCache::new(meter),
            provider: Box::new(provider),
            redaction_engine,
        }
    }
}

impl Drop for MetricEventProcessor {
    fn drop(&mut self) {
        let _ = self.provider.force_flush();
    }
}

impl std::fmt::Debug for MetricEventProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricEventProcessor").finish_non_exhaustive()
    }
}

impl EventProcessor for MetricEventProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        !description.is_disabled() && description.contains_metrics()
    }

    fn process(&self, event: &EventView<'_>) {
        let engine = &self.redaction_engine;
        let description = event.description();
        let event_metric = description.metric();
        let mut field_metrics: Vec<(observed::metadata::MetricDescription, f64)> = Vec::new();
        let mut dimensions: Vec<opentelemetry::KeyValue> = Vec::new();

        // Visit event fields once, collecting metric values and shared dimensions.
        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(metric) = desc.metric() {
                if let Some(instrument) = metric.instrument_description() {
                    if let Some(number) = observed_utils::metric_number_of(&get_value(engine)) {
                        field_metrics.push((*instrument, number));
                    }
                } else {
                    let value = get_value(engine);
                    dimensions.push(opentelemetry::KeyValue::new(
                        opentelemetry::Key::from(metric.key().to_owned()),
                        observed_utils::otel_value_of(value),
                    ));
                }
            }

            ControlFlow::Continue(())
        });

        if field_metrics.is_empty() && event_metric.is_none() {
            return;
        }

        // Visit enrichments once, collecting any that opted in as metric
        // dimensions via `#[dimension]`.
        let _ = event.visit_enrichments(&mut |desc, get_value| {
            if let Some(metric) = desc.metric() {
                let value = get_value(engine);
                dimensions.push(opentelemetry::KeyValue::new(
                    opentelemetry::Key::from(metric.key().to_owned()),
                    observed_utils::otel_value_of(value),
                ));
            }
            ControlFlow::Continue(())
        });

        for (metric, value) in field_metrics {
            self.instruments.record(&metric, value, &dimensions);
        }

        if let Some(metric) = event_metric {
            self.instruments.record(metric, 1.0, &dimensions);
        }
    }

    fn flush(&self) -> Result<(), FlushError> {
        self.provider.force_flush().map_err(|e| FlushError::new("otel-metrics", e))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error;
    use std::ops::ControlFlow;
    use std::time::SystemTime;

    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use data_privacy::{DataClass, RedactionEngine, Sensitive};
    use observed::enrichment::EnrichFnExt;
    use observed::interop::{DynEvent, emit_dyn_event};
    use observed::metadata::{EventDescription, FieldDescriptor, InstrumentKind, MetricDescription, MetricFieldEntry};
    use observed::processing::{EventProcessor, EventView, FieldVisitorFn};
    use observed::{Enrichment, Event, SinkId, Value, emit, event};
    use observed_destination_memory::memory_metrics;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    use crate::MetricEventProcessor;
    use crate::test_support::{LogOnly, TestRequestCount, UnsourcedEvent};

    #[test]
    fn metric_processor_ignores_events_without_metrics() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        let processor = MetricEventProcessor::new(provider, RedactionEngine::builder().build(), None);

        // Bypass Sink's interest check to reach the processor's no-metrics return.
        processor.process(&EventView::new_synthetic(
            &UnsourcedEvent(LogOnly::DESCRIPTION),
            SystemTime::UNIX_EPOCH,
        ));
        processor.flush().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();
        assert!(
            metrics
                .iter()
                .flat_map(ResourceMetrics::scope_metrics)
                .flat_map(ScopeMetrics::metrics)
                .next()
                .is_none()
        );
    }

    #[test]
    fn metric_processor_drop_keeps_a_retained_provider_running() {
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(InMemoryMetricExporter::default())
            .build();
        let processor = MetricEventProcessor::new(provider.clone(), RedactionEngine::builder().build(), None);

        drop(processor);

        provider.force_flush().unwrap();
        provider.shutdown().unwrap();
    }

    #[test]
    fn metric_processor_flush_preserves_provider_failures() {
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(InMemoryMetricExporter::default())
            .build();
        let processor = MetricEventProcessor::new(provider.clone(), RedactionEngine::builder().build(), None);
        assert!(format!("{processor:?}").contains("MetricEventProcessor"));

        // A shut-down provider rejects flushes without a custom exporter.
        provider.shutdown().unwrap();

        let error = processor.flush().unwrap_err();

        assert_eq!(error.processor(), "otel-metrics");
        assert!(error.source().is_some());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn redacted_dynamic_measurement_is_not_recorded_as_a_number() {
        struct DynamicMetricEvent {
            classified_duration_ms: Sensitive<f64>,
            control_duration_ms: f64,
        }

        impl DynEvent for DynamicMetricEvent {
            fn name(&self) -> &'static str {
                "test.dynamic_metric"
            }

            fn body(&self) -> Option<Cow<'static, str>> {
                None
            }

            fn source_file(&self) -> Option<Cow<'static, str>> {
                None
            }

            fn source_line(&self) -> Option<u32> {
                None
            }

            fn source_crate(&self) -> Option<Cow<'static, str>> {
                None
            }

            fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
                let classified = FieldDescriptor::new(
                    "classified_duration_ms",
                    None,
                    Some(MetricFieldEntry::instrument(
                        "classified_duration_ms",
                        MetricDescription::new("test.dynamic.classified", InstrumentKind::Histogram, "", "ms"),
                    )),
                );
                visitor(&classified, &|redactor| {
                    Value::from_redacted(&self.classified_duration_ms, redactor)
                })?;

                let control = FieldDescriptor::new(
                    "control_duration_ms",
                    None,
                    Some(MetricFieldEntry::instrument(
                        "control_duration_ms",
                        MetricDescription::new("test.dynamic.control", InstrumentKind::Histogram, "", "ms"),
                    )),
                );
                visitor(&control, &|_| Value::from(self.control_duration_ms))
            }

            fn description(&self) -> EventDescription {
                EventDescription::new(self.name(), None, None, None, true, false)
            }
        }

        const TEST_DC: DataClass = DataClass::new("test", "classified_metric");

        // Classified values become text even when redaction passes their contents through.
        for mode in [SimpleRedactorMode::Erase, SimpleRedactorMode::Passthrough] {
            let exporter = InMemoryMetricExporter::default();
            let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
            let redaction_engine = RedactionEngine::builder()
                .add_class_redactor(TEST_DC, SimpleRedactor::with_mode(mode))
                .build();
            let sink = crate::builder()
                .processor(MetricEventProcessor::new(provider, redaction_engine, None))
                .clock(tick::SimpleClock::new_frozen())
                .build()
                .unwrap();

            emit_dyn_event(
                &sink,
                &DynamicMetricEvent {
                    classified_duration_ms: Sensitive::new(123.5, TEST_DC),
                    control_duration_ms: 42.5,
                },
            );
            sink.flush().unwrap();

            let batches = exporter.get_finished_metrics().unwrap();
            let metrics: Vec<_> = batches
                .iter()
                .flat_map(ResourceMetrics::scope_metrics)
                .flat_map(ScopeMetrics::metrics)
                .collect();
            assert_eq!(metrics.len(), 1);
            assert_eq!(metrics[0].name(), "test.dynamic.control");

            let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metrics[0].data() else {
                panic!("expected f64 histogram, got: {:?}", metrics[0].data());
            };
            let data_points: Vec<_> = histogram.data_points().collect();
            assert_eq!(data_points.len(), 1);
            assert_eq!(data_points[0].count(), 1);
            assert!((data_points[0].sum() - 42.5).abs() < 1e-9);
            assert!(data_points[0].attributes().next().is_none());
        }
    }

    #[event("test.request")]
    #[info("Test request")]
    #[histogram(duration_ms, name = "test.request.duration")]
    struct TestRequest {
        #[unredacted]
        duration_ms: f64,
        #[dimension(metric = "status")]
        #[unredacted]
        status: i64,
        #[dimension(metric = "request_id")]
        #[unredacted]
        request_id: i64,
    }

    #[event("test.multi_metric")]
    #[info]
    #[histogram(duration_ms, name = "test.multi_metric.duration")]
    #[gauge(queue_depth, name = "test.multi_metric.queue_depth")]
    struct TestMultiMetric {
        #[unredacted]
        duration_ms: f64,
        #[unredacted]
        queue_depth: f64,
        #[dimension(metric = "status")]
        #[unredacted]
        status: i64,
    }

    #[derive(Enrichment)]
    struct TestEnrichment {
        // Opts in as a metric dimension via `#[dimension(metric = "...")]`.
        #[dimension(metric = "tenant_id")]
        #[unredacted]
        tenant_id: i64,
        // No `#[dimension]`: a log attribute only, never a metric dimension.
        #[unredacted]
        trace_id: i64,
    }

    fn emit_test_request(exporter: &InMemoryMetricExporter) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
        let sink = crate::builder()
            .metric(memory_metrics(exporter.clone()))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        emit!(
            sink,
            TestRequest {
                duration_ms: 42.5,
                status: 200,
                request_id: 99999,
            }
        );
        sink.flush().unwrap();

        exporter.get_finished_metrics().expect("should get metrics")
    }

    fn emit_test_request_count(exporter: &InMemoryMetricExporter) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
        let sink = crate::builder()
            .metric(memory_metrics(exporter.clone()))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        emit!(sink, TestRequestCount { status: 200 });
        sink.flush().unwrap();

        exporter.get_finished_metrics().expect("should get metrics")
    }

    fn emit_test_multi_metric(exporter: &InMemoryMetricExporter) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
        let sink = crate::builder()
            .metric(memory_metrics(exporter.clone()))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        emit!(
            sink,
            TestMultiMetric {
                duration_ms: 42.5,
                queue_depth: 7.0,
                status: 200,
            }
        );
        sink.flush().unwrap();

        exporter.get_finished_metrics().expect("should get metrics")
    }

    fn emit_test_request_with_enrichment(exporter: &InMemoryMetricExporter) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
        let sink = crate::builder()
            .metric(memory_metrics(exporter.clone()))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        (|| {
            emit!(
                sink,
                TestRequest {
                    duration_ms: 42.5,
                    status: 200,
                    request_id: 99999,
                }
            );
        })
        .enrich(
            &sink,
            TestEnrichment {
                tenant_id: 7,
                trace_id: 123,
            },
        )();
        sink.flush().unwrap();

        exporter.get_finished_metrics().expect("should get metrics")
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn metric_event_is_recorded_via_meter() {
        let metrics = emit_test_request(&InMemoryMetricExporter::default());

        assert!(!metrics.is_empty(), "expected at least one ResourceMetrics batch, got none");

        let metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == "test.request.duration")
            .expect("expected test.request.duration metric");

        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
            panic!("expected f64 histogram metric, got: {:?}", metric.data());
        };
        let data_points: Vec<_> = histogram.data_points().collect();
        assert_eq!(data_points.len(), 1, "expected one histogram data point");

        let data_point = data_points[0];
        assert_eq!(data_point.count(), 1, "expected single histogram sample");
        assert!(
            (data_point.sum() - 42.5).abs() < 1e-9,
            "expected histogram sum to be 42.5, got {}",
            data_point.sum()
        );

        let attrs: Vec<_> = data_point.attributes().cloned().collect();
        assert!(attrs.contains(&KeyValue::new("status", 200i64)));
        assert!(attrs.contains(&KeyValue::new("request_id", 99999i64)));
        assert!(!attrs.iter().any(|attr| attr.key.as_str() == "duration_ms"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn event_level_metric_without_field_is_recorded_via_meter() {
        let metrics = emit_test_request_count(&InMemoryMetricExporter::default());

        assert!(!metrics.is_empty(), "expected at least one ResourceMetrics batch, got none");

        let metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == "test.request.count")
            .expect("expected test.request.count metric");

        let AggregatedMetrics::F64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected f64 sum metric, got: {:?}", metric.data());
        };
        let data_points: Vec<_> = sum.data_points().collect();
        assert_eq!(data_points.len(), 1, "expected one counter data point");

        let data_point = data_points[0];
        assert!(
            (data_point.value() - 1.0).abs() < 1e-9,
            "expected event-level metric value of 1.0, got {}",
            data_point.value()
        );

        assert!(data_point.attributes().any(|a| a == &KeyValue::new("status", 200i64)));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn multi_metric_event_reuses_collection_without_changing_dimensions() {
        let metrics = emit_test_multi_metric(&InMemoryMetricExporter::default());

        let duration_metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == "test.multi_metric.duration")
            .expect("expected duration metric");

        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = duration_metric.data() else {
            panic!("expected histogram metric, got: {:?}", duration_metric.data());
        };
        let data_point = histogram.data_points().next().expect("expected one histogram data point");
        let attrs: Vec<_> = data_point.attributes().cloned().collect();

        assert!((data_point.sum() - 42.5).abs() < 1e-9);
        assert_eq!(attrs, [KeyValue::new("status", 200i64)]);

        let queue_depth_metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == "test.multi_metric.queue_depth")
            .expect("expected queue depth metric");

        let AggregatedMetrics::F64(MetricData::Gauge(gauge)) = queue_depth_metric.data() else {
            panic!("expected gauge metric, got: {:?}", queue_depth_metric.data());
        };
        let data_point = gauge.data_points().next().expect("expected one gauge data point");
        let attrs: Vec<_> = data_point.attributes().cloned().collect();

        assert!((data_point.value() - 7.0).abs() < 1e-9);
        assert_eq!(attrs, [KeyValue::new("status", 200i64)]);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock when metric aggregation starts. This test runs natively and under cargo careful."
    )]
    fn enrichment_dimension_field_is_recorded_as_dimension() {
        let metrics = emit_test_request_with_enrichment(&InMemoryMetricExporter::default());

        let metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == "test.request.duration")
            .expect("expected test.request.duration metric");

        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
            panic!("expected f64 histogram metric, got: {:?}", metric.data());
        };
        let data_point = histogram.data_points().next().expect("expected one histogram data point");
        let attrs: Vec<_> = data_point.attributes().cloned().collect();
        // Event field dimension still present.
        assert!(attrs.contains(&KeyValue::new("status", 200i64)));
        // Enrichment field that opted in via `#[dimension]` is a dimension.
        assert!(attrs.contains(&KeyValue::new("tenant_id", 7i64)));
        // Enrichment field without `#[dimension]` is NOT a dimension.
        assert!(!attrs.iter().any(|attr| attr.key.as_str() == "trace_id"));
    }

    #[test]
    fn disabled_event_is_not_emitted_as_metric() {
        #[event("test.disabled_metric", disabled)]
        #[info]
        #[counter]
        struct DisabledMetricEvent {
            #[dimension(metric = "dim")]
            #[unredacted]
            dim: i64,
        }

        static ID: SinkId = SinkId::new("disabled_metric_test");

        let exporter = InMemoryMetricExporter::default();
        let exporter_clone = exporter.clone();
        let sink = crate::builder()
            .id(ID)
            .metric(memory_metrics(exporter))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        emit!(sink, DisabledMetricEvent { dim: 1 });

        sink.flush().expect("flush should succeed");

        let metrics = exporter_clone.get_finished_metrics().expect("should get metrics");

        let has_disabled_metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .any(|m| m.name() == "test.disabled_metric");
        assert!(!has_disabled_metric, "disabled event should not produce metrics");
    }
}
