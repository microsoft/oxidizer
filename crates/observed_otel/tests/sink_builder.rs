// Copyright (c) Microsoft Corporation.

#![allow(
    clippy::unwrap_used,
    reason = "Non-production code uses unwrap to fail immediately when a test or demonstration cannot proceed"
)]

//! Integration tests for [`observed_otel::Builder`]'s `.log()` / `.metric()`
//! destination path and the accompanying [`observed::Sink::flush`] /
//! `Drop`-based flush semantics.

use std::any::type_name;
use std::error::Error;
use std::fmt::{self, Display};

use data_privacy::DataClass;
use observed::enrichment::EnrichFnExt;
use observed::{
    Enrichment, EventSampler, EventSamplingContext, EventSamplingDecision, Sink, SinkId, emit,
    event,
};
use observed_destination_core::{DestinationResult, LogDestination};
use observed_destination_memory::{memory_logs, memory_metrics};
use observed_otel::{
    ErasedLogDestination, ErasedMetricDestination, LogDestinationExt, MetricDestinationExt,
    RedactionPolicy, processor_builder,
};
use opentelemetry::{Key, KeyValue, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{InMemoryLogExporter, LoggerProviderBuilder};
use opentelemetry_sdk::metrics::InMemoryMetricExporter;

const DC: DataClass = DataClass::new("test", "public");

#[event("test.request")]
#[info("Test request")]
#[histogram(duration_ms, name = "test.request.duration")]
struct TestRequest {
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    duration_ms: f64,
    #[data_class(DC)]
    status: i64,
    #[data_class(DC)]
    request_id: i64,
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OTel InMemoryLogExporter calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
)]
// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
fn log_and_metric_destinations_keep_independent_resource_overlays() {
    static ID: SinkId = SinkId::new("builder_log_metric");

    let log_exporter = InMemoryLogExporter::default();
    let metric_exporter = InMemoryMetricExporter::default();
    let baseline = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", "baseline"),
            KeyValue::new("deployment.environment.name", "test"),
        ])
        .build();
    // A disjoint key distinguishes replacement from merging successive overlays.
    let obsolete = Resource::builder_empty()
        .with_attribute(KeyValue::new("obsolete", true))
        .build();
    let log_resource = Resource::builder_empty()
        .with_attribute(KeyValue::new("service.name", "logs"))
        .build();
    let metric_resource = Resource::builder_empty()
        .with_attribute(KeyValue::new("service.name", "metrics"))
        .build();

    let log_destination = memory_logs(log_exporter.clone())
        .build()
        .extend_resource(obsolete.clone())
        .extend_resource(log_resource);

    assert!(format!("{log_destination:?}").contains("MemoryLogDestination"));

    let log_destination = ErasedLogDestination::from(log_destination);

    assert!(format!("{log_destination:?}").contains("logs"));

    let metric_destination = memory_metrics(metric_exporter.clone())
        .build()
        .extend_resource(obsolete)
        .extend_resource(metric_resource);
    let metric_destination = ErasedMetricDestination::from(metric_destination);

    assert!(format!("{metric_destination:?}").contains("metrics"));

    let builder = observed_otel::builder()
        .id(ID)
        .resource(baseline)
        .log(log_destination)
        .metric(metric_destination)
        .redaction(RedactionPolicy::new())
        .clock(tick::SimpleClock::new_frozen());

    assert!(format!("{builder:?}").contains("builder_log_metric"));

    let sink = builder.build().unwrap();

    assert_eq!(sink.id(), ID);
    emit!(
        sink,
        TestRequest {
            duration_ms: 2.5,
            status: 200,
            request_id: 1
        }
    );
    sink.flush().unwrap();

    let logs = log_exporter.get_emitted_logs().unwrap();
    let metrics = metric_exporter.get_finished_metrics().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(metrics.len(), 1);
    for (resource, service) in [
        (logs[0].resource.as_ref(), "logs"),
        (metrics[0].resource(), "metrics"),
    ] {
        assert_eq!(
            resource.get(&Key::from_static_str("service.name")),
            Some(service.into())
        );
        assert_eq!(
            resource.get(&Key::from_static_str("deployment.environment.name")),
            Some("test".into()),
        );
        assert!(resource.get(&Key::from_static_str("obsolete")).is_none());
    }
}

#[test]
fn destination_registration_preserves_initialization_errors() {
    #[derive(Debug)]
    struct InitializationError {
        code: u32,
    }

    impl Display for InitializationError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}: {}", type_name::<Self>(), self.code)
        }
    }

    impl Error for InitializationError {}

    #[derive(Debug)]
    struct FailingLogDestination(InitializationError);

    impl LogDestination for FailingLogDestination {
        fn register(
            self,
            _builder: LoggerProviderBuilder,
        ) -> DestinationResult<LoggerProviderBuilder> {
            Err(Box::new(self.0))
        }
    }

    // Fail at registration without depending on any concrete destination's I/O.
    let error = observed_otel::builder()
        .log(FailingLogDestination(InitializationError { code: 42 }))
        .build()
        .unwrap_err();

    // The builder must preserve both the concrete error type and its payload.
    let error = error.downcast_ref::<InitializationError>().unwrap();
    assert_eq!(error.code, 42);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "FIXME: pass tick::Clock into Sink so EventView timestamps do not call SystemTime::now() under Miri isolation"
)]
fn metric_description_and_unit_reach_otel_exporter() {
    use observed_otel::builder;
    use opentelemetry_sdk::metrics::data::{ResourceMetrics, ScopeMetrics};
    use tick::SimpleClock;

    #[event("test.documented")]
    #[info("Documented metric")]
    #[histogram(
        size_bytes,
        name = "test.request.size",
        desc = "Request payload size",
        unit = "By"
    )]
    struct TestDocumentedMetric {
        #[unredacted]
        size_bytes: f64,
    }

    let exporter = InMemoryMetricExporter::default();
    let sink = builder()
        .metric(memory_metrics(exporter.clone()))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(sink, TestDocumentedMetric { size_bytes: 128.0 });
    sink.flush().unwrap();

    let metrics = exporter.get_finished_metrics().unwrap();
    let metric = metrics
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .find(|m| m.name() == "test.request.size")
        .expect("expected test.request.size metric");

    assert_eq!(metric.description(), "Request payload size");
    assert_eq!(metric.unit(), "By");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "FIXME: pass tick::Clock into Sink so EventView timestamps do not call SystemTime::now() under Miri isolation"
)]
fn flush_pushes_metrics_to_exporter() {
    let exporter = InMemoryMetricExporter::default();
    let exporter_clone = exporter.clone();

    let sink = observed_otel::builder()
        .metric(memory_metrics(exporter))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(
        sink,
        TestRequest {
            duration_ms: 42.5,
            status: 200,
            request_id: 99999,
        }
    );

    sink.flush().unwrap();

    let metrics = exporter_clone.get_finished_metrics().unwrap();

    let found = metrics.iter().any(|rm| {
        rm.scope_metrics()
            .any(|sm| sm.metrics().any(|m| m.name() == "test.request.duration"))
    });
    assert!(
        found,
        "expected to find test.request.duration histogram, got: {metrics:?}"
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "FIXME: pass tick::Clock into Sink so EventView timestamps do not call SystemTime::now() under Miri isolation"
)]
fn drop_flushes_metrics_to_exporter() {
    let exporter = InMemoryMetricExporter::default();
    let exporter_clone = exporter.clone();

    {
        let sink = observed_otel::builder()
            .metric(memory_metrics(exporter))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .unwrap();

        emit!(
            sink,
            TestRequest {
                duration_ms: 42.5,
                status: 200,
                request_id: 99999,
            }
        );
        // No explicit flush: processor Drop flushes before releasing its provider.
    }

    let metrics = exporter_clone.get_finished_metrics().unwrap();

    let found = metrics.iter().any(|rm| {
        rm.scope_metrics()
            .any(|sm| sm.metrics().any(|m| m.name() == "test.request.duration"))
    });
    assert!(
        found,
        "expected Drop-based flush to deliver metrics, got: {metrics:?}"
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "FIXME: pass tick::Clock into Sink so EventView timestamps do not call SystemTime::now() under Miri isolation"
)]
fn flush_is_idempotent_and_non_terminating() {
    let exporter = InMemoryMetricExporter::default();
    let sink = observed_otel::builder()
        .metric(memory_metrics(exporter))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    // Flush is safe to call many times. The sink remains usable
    // afterwards — only `Drop` terminates the providers.
    sink.flush().unwrap();
    sink.flush().unwrap();
    sink.flush().unwrap();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OTel InMemoryLogExporter calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
)]
// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
fn raw_processors_receive_events_and_flush() {
    // Noop sink has no providers; flush returns Ok.
    let noop = Sink::noop();
    noop.flush().unwrap();

    let first = InMemoryLogExporter::default();
    let second = InMemoryLogExporter::default();
    let processors = processor_builder()
        .log(memory_logs(first.clone()))
        .log(memory_logs(second.clone()))
        .build()
        .unwrap();
    let raw = observed_otel::builder()
        .processors(processors)
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(
        raw,
        TestRequest {
            duration_ms: 1.0,
            status: 200,
            request_id: 1
        }
    );
    raw.flush().unwrap();

    assert_eq!(first.get_emitted_logs().unwrap().len(), 1);
    assert_eq!(second.get_emitted_logs().unwrap().len(), 1);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "FIXME: pass tick::Clock into Sink so EventView timestamps do not call SystemTime::now() under Miri isolation"
)]
fn composite_of_two_emitters_fans_out_to_both() {
    static APP: SinkId = SinkId::new("composite_fanout_app");
    static AUX: SinkId = SinkId::new("composite_fanout_aux");

    #[event("fanout.event")]
    #[info("fanout")]
    struct FanoutEvent {
        #[unredacted]
        value: i64,
    }

    let app_exporter = InMemoryLogExporter::default();
    let app_exporter_clone = app_exporter.clone();
    let aux_exporter = InMemoryLogExporter::default();
    let aux_exporter_clone = aux_exporter.clone();

    let app = observed_otel::builder()
        .id(APP)
        .log(memory_logs(app_exporter))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();
    let aux = observed_otel::builder()
        .id(AUX)
        .log(memory_logs(aux_exporter))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    let both = Sink::composite([app.clone(), aux.clone()]);

    emit!(both, FanoutEvent { value: 1 });

    // Flush both. Dropping the composite does nothing special — each child
    // flushes via its own `flush()` / `Drop`.
    app.flush().unwrap();
    aux.flush().unwrap();

    let app_logs = app_exporter_clone.get_emitted_logs().unwrap();
    let aux_logs = aux_exporter_clone.get_emitted_logs().unwrap();

    assert!(
        !app_logs.is_empty(),
        "app destinations should have received the event via the composite"
    );
    assert!(
        !aux_logs.is_empty(),
        "aux destinations should have received the event via the composite"
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OTel InMemoryLogExporter calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
)]
// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
fn composite_enrichment_respects_child_isolation_and_scope() {
    use opentelemetry_sdk::logs::SdkLogRecord;

    static APP: SinkId = SinkId::new("composite_enrichment_app");
    static AUX: SinkId = SinkId::new("composite_enrichment_aux");

    fn context_keys(record: &SdkLogRecord) -> Vec<&str> {
        record
            .attributes_iter()
            .map(|(key, _)| key.as_str())
            .filter(|key| matches!(*key, "global" | "targeted"))
            .collect()
    }

    #[event("fanout.event")]
    #[info("fanout")]
    struct FanoutEvent;

    #[derive(Enrichment)]
    struct GlobalContext {
        #[unredacted]
        global: bool,
    }

    #[derive(Enrichment)]
    struct TargetedContext {
        #[unredacted]
        targeted: bool,
    }

    let app_exporter = InMemoryLogExporter::default();
    let aux_exporter = InMemoryLogExporter::default();

    let app = observed_otel::builder()
        .id(APP)
        .log(memory_logs(app_exporter.clone()))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();
    let aux = observed_otel::builder()
        .id(AUX)
        .isolated_enrichment()
        .log(memory_logs(aux_exporter.clone()))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    let both = Sink::composite([app, aux]);

    (|| emit!(both, FanoutEvent))
        .enrich_for(&both, AUX, TargetedContext { targeted: true })
        .enrich(&both, GlobalContext { global: true })();

    emit!(both, FanoutEvent);

    // Memory destinations export synchronously, so no flush is needed.
    let app_logs = app_exporter.get_emitted_logs().unwrap();
    let aux_logs = aux_exporter.get_emitted_logs().unwrap();
    assert_eq!(app_logs.len(), 2);
    assert_eq!(aux_logs.len(), 2);

    // Each child sees only the context allowed by its enrichment policy.
    assert_eq!(context_keys(&app_logs[0].record), ["global"]);
    assert_eq!(context_keys(&aux_logs[0].record), ["targeted"]);

    // Neither child's context may survive beyond the enrichment scope.
    assert!(context_keys(&app_logs[1].record).is_empty());
    assert!(context_keys(&aux_logs[1].record).is_empty());
}

/// A destination with `.extend_redaction(passthrough_for_TEST_DC)` records
/// values classified as `TEST_DC` verbatim, while another destination on
/// the same sink (no overlay) records them as erased — proving the
/// overlay merges on top of the baseline only for the destination that
/// declared it.
#[test]
#[cfg_attr(
    miri,
    ignore = "OTel InMemoryLogExporter calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
)]
// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
fn extend_redaction_applies_only_to_that_log_destination() {
    use data_privacy::Sensitive;
    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use opentelemetry::logs::AnyValue;
    use opentelemetry_sdk::logs::SdkLogRecord;

    fn read_secret(record: &SdkLogRecord) -> &AnyValue {
        &record
            .attributes_iter()
            .find(|(key, _)| key.as_str() == "secret")
            .unwrap()
            .1
    }

    #[event("redacted.event")]
    #[info("test")]
    struct RedactedEvent {
        secret: Sensitive<&'static str>,
    }

    const TEST_DC: DataClass = DataClass::new("test", "sensitive");

    let erase_exporter = InMemoryLogExporter::default();
    let erase_clone = erase_exporter.clone();
    let passthrough_exporter = InMemoryLogExporter::default();
    let passthrough_clone = passthrough_exporter.clone();

    let baseline = RedactionPolicy::new().class(
        TEST_DC,
        SimpleRedactor::with_mode(SimpleRedactorMode::Erase),
    );
    let passthrough_overlay = RedactionPolicy::new().class(
        TEST_DC,
        SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
    );

    let sink = observed_otel::builder()
        .redaction(baseline)
        .log(memory_logs(erase_exporter)) // inherits baseline (erase)
        .log(
            memory_logs(passthrough_exporter)
                .extend_redaction(RedactionPolicy::new()) // should be ignored due to next line
                .extend_redaction(passthrough_overlay), // testing overlaying capability
        )
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(
        sink,
        RedactedEvent {
            secret: Sensitive::new("plaintext-secret", TEST_DC),
        }
    );

    let erase_logs = erase_clone.get_emitted_logs().unwrap();
    let passthrough_logs = passthrough_clone.get_emitted_logs().unwrap();

    assert_eq!(erase_logs.len(), 1);
    assert_eq!(passthrough_logs.len(), 1);
    assert_eq!(
        read_secret(&passthrough_logs[0].record),
        &AnyValue::String("plaintext-secret".into())
    );
    assert_eq!(
        read_secret(&erase_logs[0].record),
        &AnyValue::String("".into())
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OTel MeterProviderBuilder eagerly calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
)]
// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
fn extend_redaction_applies_only_to_that_metric_destination() {
    use data_privacy::Sensitive;
    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use observed_otel::builder;
    use tick::SimpleClock;

    #[event("redacted.event")]
    #[counter]
    struct RedactedEvent {
        #[dimension(metric = "secret")]
        secret: Sensitive<&'static str>,
    }

    const TEST_DC: DataClass = DataClass::new("test", "sensitive");

    let erase_metrics = InMemoryMetricExporter::default();
    let passthrough_metrics = InMemoryMetricExporter::default();

    let baseline = RedactionPolicy::new().class(
        TEST_DC,
        SimpleRedactor::with_mode(SimpleRedactorMode::Erase),
    );
    let passthrough_overlay = RedactionPolicy::new().class(
        TEST_DC,
        SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
    );

    let sink = builder()
        .redaction(baseline)
        .metric(memory_metrics(erase_metrics.clone()))
        .metric(
            memory_metrics(passthrough_metrics.clone())
                .extend_redaction(RedactionPolicy::new())
                .extend_redaction(passthrough_overlay),
        )
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(
        sink,
        RedactedEvent {
            secret: Sensitive::new("plaintext-secret", TEST_DC),
        }
    );
    sink.flush().unwrap();

    assert_eq!(
        read_metric_secret(&passthrough_metrics),
        Value::from("plaintext-secret")
    );
    assert_eq!(read_metric_secret(&erase_metrics), Value::from(""));
}

fn read_metric_secret(exporter: &InMemoryMetricExporter) -> Value {
    use opentelemetry_sdk::metrics::data::{
        AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
    };

    let metrics = exporter.get_finished_metrics().unwrap();
    let metric = metrics
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == "redacted.event")
        .unwrap();
    let AggregatedMetrics::F64(MetricData::Sum(sum)) = metric.data() else {
        panic!("expected counter");
    };
    let mut points = sum.data_points();
    let point = points.next().unwrap();
    assert!(points.next().is_none());

    point
        .attributes()
        .find(|attribute| attribute.key.as_str() == "secret")
        .unwrap()
        .value
        .clone()
}

#[event("health.check")]
#[info("Health check")]
struct HealthCheck;

/// Drops events with this name and continues all other events.
struct DropEvent(&'static str);

impl EventSampler for DropEvent {
    fn sample(&self, context: &EventSamplingContext<'_>) -> EventSamplingDecision {
        if context.description().name() == self.0 {
            EventSamplingDecision::Drop
        } else {
            EventSamplingDecision::Continue
        }
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "OTel InMemoryLogExporter calls current_exe (readlink/GetModuleFileNameW), unsupported by Miri isolation"
)]
// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
fn second_builder_event_sampler_replaces_first() {
    let logs = InMemoryLogExporter::default();
    let sink = observed_otel::builder()
        .event_sampler(DropEvent("test.request"))
        .event_sampler(DropEvent("health.check"))
        .log(memory_logs(logs.clone()))
        .clock(tick::SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(sink, HealthCheck);
    emit!(
        sink,
        TestRequest {
            duration_ms: 1.0,
            status: 200,
            request_id: 1
        }
    );

    let log_names: Vec<_> = logs
        .get_emitted_logs()
        .unwrap()
        .iter()
        .map(|log| log.record.event_name().unwrap())
        .collect();
    assert_eq!(log_names, ["test.request"]);
}
