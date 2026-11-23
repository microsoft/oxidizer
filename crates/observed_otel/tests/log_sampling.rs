// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests how sinks sample, deliver, and flush log records.

#![allow(
    clippy::unwrap_used,
    reason = "Non-production code uses unwrap to fail immediately when a test or demonstration cannot proceed"
)]

use std::time::{Duration, SystemTime};
use std::{fmt, mem, thread};

use data_privacy::{DataClass, Redactor, Sensitive};
use observed::enrichment::EnrichFnExt;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{Enrichment, FlushError, Sink, emit, event};
use observed_destination_core::{DestinationResult, LogDestination};
use observed_destination_memory::memory_metrics;
use observed_otel::sampling::{LogSampler, LogSamplingContext, SamplingBatch};
use observed_otel::{LogDestinationExt, RedactionPolicy, builder, processor_builder};
use opentelemetry::baggage::BaggageExt;
use opentelemetry::logs::{AnyValue, LogRecord, Severity};
use opentelemetry::trace::{SpanContext, TraceContextExt, TraceState};
use opentelemetry::{Context, InstrumentationScope, KeyValue, SpanId, TraceFlags, TraceId};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{LogProcessor, LoggerProviderBuilder, SdkLogRecord};
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics};
use performables::arc::{Arc, Arc as PerformablesArc};
use performables::sync::channel as mpsc;
use performables::sync::mutex::Mutex;
use tick::SimpleClock;

#[event("sampling.message")]
#[info("Message")]
struct Message {
    #[unredacted]
    value: i64,
}

#[event("sampling.counter")]
#[info]
#[counter]
struct CountedMessage;

const PRIVATE: DataClass = DataClass::new("sampling", "private");

#[event("sampling.secret")]
#[info("Secret message")]
struct SecretMessage<'a> {
    secret: Sensitive<&'a str>,
}

#[derive(Enrichment)]
struct SecretEnrichment {
    enrichment: Sensitive<String>,
}

struct FixedRedactor(&'static str);

impl Redactor for FixedRedactor {
    fn redacts(&self, _: &DataClass) -> bool {
        true
    }

    fn redact(&self, _: &DataClass, _: &str, output: &mut dyn fmt::Write) -> fmt::Result {
        output.write_str(self.0)
    }
}

#[derive(Debug)]
struct TaggingDestination(LogProbe);

impl LogDestination for TaggingDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        let probe = self.0.clone();
        Ok(builder.with_log_processor(self).with_log_processor(probe))
    }
}

impl LogProcessor for TaggingDestination {
    fn emit(&self, data: &mut SdkLogRecord, _: &InstrumentationScope) {
        assert!(data.observed_timestamp().is_some());
        assert_eq!(data.body(), Some(&AnyValue::String("Message".into())));
        data.set_body("processed".into());
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
}

struct RequestMarker(&'static str);

struct SuppressedProcessor {
    processor: PerformablesArc<dyn EventProcessor>,
    suppressed: Arc<Mutex<bool>>,
}

impl EventProcessor for SuppressedProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        self.processor.is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        let suppressed = *self.suppressed.lock();
        let context = Context::current();
        let _guard = if suppressed { context.with_telemetry_suppressed() } else { context }.attach();
        self.processor.process(event);
        assert_eq!(Context::is_current_telemetry_suppressed(), suppressed);
    }

    fn flush(&self) -> Result<(), FlushError> {
        self.processor.flush()
    }
}

#[derive(Debug)]
struct Observation {
    record: SdkLogRecord,
    context: Context,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LifecycleCall {
    SamplerFlush,
    Emit,
    ProviderFlush,
    SamplerDrop,
    ProviderShutdown,
}

#[derive(Clone, Debug, Default)]
struct LogProbe {
    observations: Arc<Mutex<Vec<Observation>>>,
    calls: Arc<Mutex<Vec<LifecycleCall>>>,
    fail_flush: bool,
}

impl LogProbe {
    fn attribute_values(&self, key: &str) -> Vec<Option<AnyValue>> {
        self.observations.lock().iter().map(|log| attribute(&log.record, key)).collect()
    }

    fn lifecycle_calls(&self) -> Vec<LifecycleCall> {
        self.calls.lock().clone()
    }
}

impl LogDestination for LogProbe {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        Ok(builder.with_log_processor(self))
    }
}

impl LogProcessor for LogProbe {
    fn emit(&self, data: &mut SdkLogRecord, _: &InstrumentationScope) {
        let observation = Context::map_current(|context| Observation {
            record: data.clone(),
            context: context.clone(),
        });
        self.observations.lock().push(observation);
        self.calls.lock().push(LifecycleCall::Emit);
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.calls.lock().push(LifecycleCall::ProviderFlush);
        if self.fail_flush {
            Err(OTelSdkError::InternalFailure("test flush failure".into()))
        } else {
            Ok(())
        }
    }

    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        self.calls.lock().push(LifecycleCall::ProviderShutdown);
        Ok(())
    }
}

struct SaveUntilFlush {
    records: Mutex<Vec<LogSamplingContext>>,
    calls: Arc<Mutex<Vec<LifecycleCall>>>,
}

impl SaveUntilFlush {
    fn new(probe: &LogProbe) -> Self {
        Self {
            records: Mutex::<_>::new(Vec::new()),
            calls: Arc::clone(&probe.calls),
        }
    }
}

impl LogSampler for SaveUntilFlush {
    fn sample(&self, log: LogSamplingContext) -> SamplingBatch<LogSamplingContext> {
        self.records.lock().push(log);
        SamplingBatch::empty()
    }

    fn flush(&self) -> SamplingBatch<LogSamplingContext> {
        self.calls.lock().push(LifecycleCall::SamplerFlush);
        SamplingBatch::many(mem::take(&mut *self.records.lock()))
    }
}

impl Drop for SaveUntilFlush {
    fn drop(&mut self) {
        self.calls.lock().push(LifecycleCall::SamplerDrop);
    }
}

struct SampleFn<F>(F);

impl<F> LogSampler for SampleFn<F>
where
    F: Fn(LogSamplingContext) -> SamplingBatch<LogSamplingContext> + Send + Sync + 'static,
{
    fn sample(&self, log: LogSamplingContext) -> SamplingBatch<LogSamplingContext> {
        (self.0)(log)
    }

    fn flush(&self) -> SamplingBatch<LogSamplingContext> {
        SamplingBatch::empty()
    }
}

fn attribute(record: &SdkLogRecord, key: &str) -> Option<AnyValue> {
    record
        .attributes_iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.clone())
}

fn assert_captured_context(observation: &Observation, request: &str, span_context: &SpanContext) {
    assert_eq!(observation.context.get::<RequestMarker>().unwrap().0, request);
    assert_eq!(observation.context.span().span_context(), span_context);
    assert_eq!(
        observation.context.baggage().get("request").map(ToString::to_string),
        Some(String::from(request))
    );
    assert!(!observation.context.is_telemetry_suppressed());
    let trace = observation.record.trace_context().unwrap();
    assert_eq!(trace.trace_id, span_context.trace_id());
    assert_eq!(trace.span_id, span_context.span_id());
    assert_eq!(trace.trace_flags, Some(span_context.trace_flags()));
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn continued_log_preserves_otel_context() {
    let span_context = SpanContext::new(
        TraceId::from(1_u128),
        SpanId::from(2_u64),
        TraceFlags::SAMPLED,
        false,
        TraceState::default(),
    );
    let sampler_span_context = span_context.clone();
    let probe = LogProbe::default();
    let sampler = SampleFn(move |log: LogSamplingContext| {
        let context = log.otel_context();
        assert_eq!(context.get::<RequestMarker>().unwrap().0, "original");
        assert_eq!(context.span().span_context(), &sampler_span_context);
        assert_eq!(
            context.baggage().get("request").map(ToString::to_string),
            Some(String::from("original"))
        );
        if attribute(log.record(), "value") == Some(AnyValue::Int(42)) {
            SamplingBatch::one(log)
        } else {
            SamplingBatch::empty()
        }
    });
    let sink = builder()
        .resource(Resource::builder_empty().build())
        .log(probe.clone().with_sampler(sampler))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    let context = Context::new()
        .with_value(RequestMarker("original"))
        .with_baggage([KeyValue::new("request", "original")])
        .with_remote_span_context(span_context.clone());
    let _guard = context.attach();

    emit!(sink, Message { value: 7 });
    emit!(sink, Message { value: 42 });

    let observations = probe.observations.lock();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].record.event_name(), Some("sampling.message"));
    assert_eq!(attribute(&observations[0].record, "value"), Some(AnyValue::Int(42)));
    assert_captured_context(&observations[0], "original", &span_context);
    assert_eq!(Context::current().get::<RequestMarker>().unwrap().0, "original");
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn unsampled_destination_preserves_ambient_context() {
    let probe = LogProbe::default();
    let sink = builder().log(probe.clone()).clock(SimpleClock::new_frozen()).build().unwrap();
    let _guard = Context::new().with_value(RequestMarker("original")).attach();

    emit!(sink, Message { value: 42 });

    let observations = probe.observations.lock();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].context.get::<RequestMarker>().unwrap().0, "original");
    assert_eq!(attribute(&observations[0].record, "value"), Some(AnyValue::Int(42)));
}

struct Discard;

impl LogSampler for Discard {
    fn sample(&self, _: LogSamplingContext) -> SamplingBatch<LogSamplingContext> {
        SamplingBatch::empty()
    }

    fn flush(&self) -> SamplingBatch<LogSamplingContext> {
        SamplingBatch::empty()
    }
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn empty_log_batch_discards_one_destination() {
    let sampled = LogProbe::default();
    let unsampled = LogProbe::default();
    let sink = builder()
        .log(sampled.clone().with_sampler(Discard))
        .log(unsampled.clone())
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(sink, Message { value: 42 });

    assert!(sampled.observations.lock().is_empty());
    let observations = unsampled.observations.lock();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].record.event_name(), Some("sampling.message"));
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn log_sampler_receives_complete_record_before_processors() {
    let probe = LogProbe::default();
    let sampled = Arc::new(Mutex::<_>::new(Vec::new()));
    let captured = Arc::clone(&sampled);
    let event_time = SystemTime::UNIX_EPOCH + Duration::from_secs(42);
    let sink = builder()
        .log(probe.clone().with_sampler(SampleFn(move |log: LogSamplingContext| {
            captured.lock().push(log.record().clone());
            SamplingBatch::one(log)
        })))
        .clock(SimpleClock::new_frozen_at(event_time))
        .build()
        .unwrap();

    emit!(sink, Message { value: 42 });

    let sampled = sampled.lock();
    assert_eq!(sampled.len(), 1);
    let record = &sampled[0];
    assert_eq!(record.event_name(), Some("sampling.message"));
    assert_eq!(record.timestamp(), Some(event_time));
    assert!(record.observed_timestamp().is_none());
    assert_eq!(record.severity_number(), Some(Severity::Info));
    assert_eq!(record.severity_text(), Some("INFO"));
    assert_eq!(record.body(), Some(&AnyValue::String("Message".into())));
    assert!(record.target().is_none());
    assert!(
        record
            .attributes_iter()
            .any(|(key, value)| key.as_str() == "value" && *value == AnyValue::Int(42))
    );
    let observations = probe.observations.lock();
    assert_eq!(observations.len(), 1);
    let mut expected = record.clone();
    expected.set_observed_timestamp(observations[0].record.observed_timestamp().unwrap());
    assert_eq!(observations[0].record, expected);
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn saved_log_preserves_context_across_suppression_changes() {
    let pending = Arc::new(Mutex::<_>::new(None::<LogSamplingContext>));
    let sampler_pending = Arc::clone(&pending);
    let sampler = SampleFn(move |log: LogSamplingContext| {
        sampler_pending
            .lock()
            .replace(log)
            .map_or_else(SamplingBatch::empty, SamplingBatch::one)
    });
    let probe = LogProbe::default();
    let processor = processor_builder()
        .log(probe.clone().with_sampler(sampler))
        .build()
        .unwrap()
        .pop()
        .unwrap();
    let suppressed = Arc::new(Mutex::<_>::new(false));
    let sink = builder()
        .processor(SuppressedProcessor {
            processor,
            suppressed: Arc::clone(&suppressed),
        })
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    {
        let _guard = Context::new().with_value(RequestMarker("original")).attach();
        emit!(sink, Message { value: 1 });
    }

    let mut expected = pending.lock().as_ref().unwrap().record().clone();
    assert!(probe.observations.lock().is_empty());

    *suppressed.lock() = true;
    {
        let _guard = Context::new().with_value(RequestMarker("release")).attach();
        {
            let pending = pending.lock();
            let context = pending.as_ref().unwrap().otel_context();
            assert_eq!(context.get::<RequestMarker>().unwrap().0, "original");
            assert!(!context.is_telemetry_suppressed());
        }
        assert_eq!(Context::current().get::<RequestMarker>().unwrap().0, "release");
        emit!(sink, Message { value: 2 });
        assert_eq!(Context::current().get::<RequestMarker>().unwrap().0, "release");
    }

    {
        let observations = probe.observations.lock();
        assert_eq!(observations.len(), 1);
        expected.set_observed_timestamp(observations[0].record.observed_timestamp().unwrap());
        assert_eq!(observations[0].record, expected);
        assert_eq!(observations[0].context.get::<RequestMarker>().unwrap().0, "original");
    }

    *suppressed.lock() = false;
    emit!(sink, Message { value: 3 });

    let observations = probe.observations.lock();
    assert_eq!(observations.len(), 1);
    assert_eq!(attribute(&observations[0].record, "value"), Some(AnyValue::Int(1)));
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn many_log_batch_preserves_contexts_across_threads() {
    let pending = Mutex::<_>::new(Vec::new());
    let sampled = LogProbe::default();
    let unsampled = LogProbe::default();
    let policy = SampleFn(move |log: LogSamplingContext| {
        let mut pending = pending.lock();
        pending.push(log);
        if pending.len() < 3 {
            SamplingBatch::empty()
        } else {
            SamplingBatch::many(mem::take(&mut *pending))
        }
    });
    let sink = builder()
        .log(sampled.clone().with_sampler(policy))
        .log(unsampled.clone())
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    for (value, request) in [(10, "first"), (20, "second"), (30, "third")] {
        thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _guard = Context::new().with_value(RequestMarker(request)).attach();
                    emit!(sink, Message { value });
                    assert_eq!(Context::current().get::<RequestMarker>().unwrap().0, request);
                })
                .join()
                .unwrap();
        });
    }

    for probe in [sampled, unsampled] {
        let observations = probe.observations.lock();
        assert_eq!(observations.len(), 3);
        for (observation, (value, request)) in observations.iter().zip([(10, "first"), (20, "second"), (30, "third")]) {
            assert_eq!(attribute(&observation.record, "value"), Some(AnyValue::Int(value)));
            assert_eq!(observation.context.get::<RequestMarker>().unwrap().0, request);
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn destination_sampler_replacement_reaches_both_builders() {
    for processors_only in [false, true] {
        let probe = LogProbe::default();
        let destination = probe
            .clone()
            .extend_resource(Resource::builder_empty().build())
            .with_sampler(Discard)
            .extend_redaction(RedactionPolicy::new())
            .with_sampler(SampleFn(SamplingBatch::one));
        let sink = if processors_only {
            Sink::builder("sampling.processors")
                .processors(
                    processor_builder()
                        .log(destination)
                        .build()
                        .unwrap()
                        .into_iter()
                        .map(PerformablesArc::into_std_arc)
                        .collect(),
                )
                .clock(SimpleClock::new_frozen())
                .build()
        } else {
            builder().log(destination).clock(SimpleClock::new_frozen()).build().unwrap()
        };

        emit!(sink, Message { value: 42 });

        let observations = probe.observations.lock();
        assert_eq!(observations.len(), 1);
        assert_eq!(attribute(&observations[0].record, "value"), Some(AnyValue::Int(42)));
    }
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn log_sampler_receives_complete_redacted_record() {
    let sampler = |expected: &'static str| {
        let pending = Mutex::<_>::new(None);
        SampleFn(move |log: LogSamplingContext| {
            for key in ["secret", "enrichment"] {
                assert_eq!(attribute(log.record(), key), Some(AnyValue::String(expected.into())));
            }
            assert_eq!(attribute(log.record(), "code.file.path"), Some(AnyValue::String(file!().into())));
            pending.lock().replace(log).map_or_else(SamplingBatch::empty, SamplingBatch::one)
        })
    };
    let baseline = LogProbe::default();
    let overlay = LogProbe::default();
    let sink = builder()
        .redaction(RedactionPolicy::new().class(PRIVATE, FixedRedactor("baseline")))
        .log(baseline.clone().with_sampler(sampler("baseline")))
        .log(
            overlay
                .clone()
                .extend_redaction(RedactionPolicy::new().class(PRIVATE, FixedRedactor("overlay")))
                .with_sampler(sampler("overlay")),
        )
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    let secret = String::from("borrowed private value");

    (|| {
        emit!(
            sink,
            SecretMessage {
                secret: Sensitive::new(secret.as_str(), PRIVATE)
            }
        );
        assert!(baseline.observations.lock().is_empty());
        assert!(overlay.observations.lock().is_empty());
        emit!(
            sink,
            SecretMessage {
                secret: Sensitive::new(secret.as_str(), PRIVATE)
            }
        );
    })
    .enrich(
        &sink,
        SecretEnrichment {
            enrichment: Sensitive::new(String::from("private enrichment"), PRIVATE),
        },
    )();
    drop(secret);

    for (probe, expected) in [(baseline, "baseline"), (overlay, "overlay")] {
        let observations = probe.observations.lock();
        assert_eq!(observations.len(), 1);
        for key in ["secret", "enrichment"] {
            assert_eq!(attribute(&observations[0].record, key), Some(AnyValue::String(expected.into())));
        }
    }
}

// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
#[test]
#[cfg_attr(miri, ignore = "OpenTelemetry resource detection uses current_exe, unsupported by isolated Miri")]
fn sink_drop_discards_saved_records_before_provider_shutdown() {
    let probe = LogProbe::default();
    let sink = builder()
        .log(probe.clone().with_sampler(SaveUntilFlush::new(&probe)))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    emit!(sink, Message { value: 42 });

    drop(sink);

    assert!(probe.observations.lock().is_empty());
    assert_eq!(
        probe.lifecycle_calls(),
        [LifecycleCall::SamplerDrop, LifecycleCall::ProviderShutdown]
    );
}

// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
#[test]
#[cfg_attr(miri, ignore = "OpenTelemetry resource detection uses current_exe, unsupported by isolated Miri")]
fn saved_record_does_not_keep_provider_alive() {
    let saved = Arc::new(Mutex::<_>::new(None));
    let sampler_saved = Arc::clone(&saved);
    let probe = LogProbe::default();
    let sink = builder()
        .log(probe.clone().with_sampler(SampleFn(move |log: LogSamplingContext| {
            *sampler_saved.lock() = Some(log);
            SamplingBatch::empty()
        })))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(sink, Message { value: 42 });
    drop(sink);

    assert_eq!(probe.lifecycle_calls(), [LifecycleCall::ProviderShutdown]);
    let log_record = saved.lock().take().unwrap();
    assert_eq!(log_record.record().event_name(), Some("sampling.message"));
    assert_eq!(attribute(log_record.record(), "value"), Some(AnyValue::Int(42)));
}

#[test]
#[cfg_attr(miri, ignore = "destination completion reads the clock and metric export uses SDK threads")]
fn late_log_sampling_does_not_change_metric_delivery() {
    let logs = LogProbe::default();
    let metrics = InMemoryMetricExporter::default();
    let sink = builder()
        .log(logs.clone().with_sampler(Discard))
        .metric(memory_metrics(metrics.clone()))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(sink, CountedMessage);
    emit!(sink, CountedMessage);
    sink.flush().unwrap();

    assert!(logs.observations.lock().is_empty());
    let metrics = metrics.get_finished_metrics().unwrap();
    let metric = metrics
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == "sampling.counter")
        .unwrap();
    let AggregatedMetrics::F64(MetricData::Sum(sum)) = metric.data() else {
        panic!("expected counter sum");
    };
    let points: Vec<_> = sum.data_points().collect();
    assert_eq!(points.len(), 1);
    assert!((points[0].value() - 2.0).abs() < f64::EPSILON);
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn processor_mutation_reaches_later_processor() {
    let probe = LogProbe::default();
    let sink = builder()
        .log(TaggingDestination(probe.clone()).with_sampler(SampleFn(SamplingBatch::one)))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    emit!(sink, Message { value: 42 });

    let observations = probe.observations.lock();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].record.body(), Some(&AnyValue::String("processed".into())));
    assert_eq!(attribute(&observations[0].record, "value"), Some(AnyValue::Int(42)));
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn flush_sends_saved_records_in_order() {
    let probe = LogProbe::default();
    let sink = builder()
        .log(probe.clone().with_sampler(SaveUntilFlush::new(&probe)))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    let span_context = SpanContext::new(
        TraceId::from(1_u128),
        SpanId::from(2_u64),
        TraceFlags::SAMPLED,
        false,
        TraceState::default(),
    );
    for (value, request) in [(1, "first"), (2, "second")] {
        let _guard = Context::new()
            .with_value(RequestMarker(request))
            .with_baggage([KeyValue::new("request", request)])
            .with_remote_span_context(span_context.clone())
            .attach();
        emit!(sink, Message { value });
    }
    assert!(probe.observations.lock().is_empty());

    sink.flush().unwrap();

    let observations = probe.observations.lock();
    assert_eq!(observations.len(), 2);
    for (observation, (value, request)) in observations.iter().zip([(1, "first"), (2, "second")]) {
        assert_eq!(attribute(&observation.record, "value"), Some(AnyValue::Int(value)));
        assert_eq!(observation.record.event_name(), Some("sampling.message"));
        assert_captured_context(observation, request, &span_context);
    }
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn flush_with_suppressed_telemetry_sends_saved_records() {
    let probe = LogProbe::default();
    let sink = builder()
        .log(probe.clone().with_sampler(SaveUntilFlush::new(&probe)))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    emit!(sink, Message { value: 1 });

    {
        let _guard = Context::new()
            .with_value(RequestMarker("flush"))
            .with_telemetry_suppressed()
            .attach();
        sink.flush().unwrap();
        assert_eq!(Context::current().get::<RequestMarker>().unwrap().0, "flush");
        assert!(Context::is_current_telemetry_suppressed());
    }

    assert_eq!(probe.attribute_values("value"), [Some(AnyValue::Int(1))]);
}

// TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
// https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
#[test]
#[cfg_attr(miri, ignore = "OpenTelemetry resource detection uses current_exe, unsupported by isolated Miri")]
fn flush_with_no_saved_records_flushes_provider() {
    let probe = LogProbe::default();
    let sink = builder()
        .log(probe.clone().with_sampler(SaveUntilFlush::new(&probe)))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();

    sink.flush().unwrap();

    assert_eq!(probe.lifecycle_calls(), [LifecycleCall::SamplerFlush, LifecycleCall::ProviderFlush]);
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn flush_sends_saved_records_before_returning_provider_error() {
    let probe = LogProbe {
        fail_flush: true,
        ..LogProbe::default()
    };
    let sink = builder()
        .log(probe.clone().with_sampler(SaveUntilFlush::new(&probe)))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    emit!(sink, Message { value: 1 });

    assert!(sink.flush().is_err());

    assert_eq!(
        probe.lifecycle_calls(),
        [LifecycleCall::SamplerFlush, LifecycleCall::Emit, LifecycleCall::ProviderFlush,]
    );
}

#[test]
#[cfg_attr(miri, ignore = "destination record completion reads the system clock")]
fn sample_runs_while_sampler_flush_runs() {
    struct PausedFlush {
        sampler: SaveUntilFlush,
        paused_sender: mpsc::Sender<()>,
        resume_receiver: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl LogSampler for PausedFlush {
        fn sample(&self, log: LogSamplingContext) -> SamplingBatch<LogSamplingContext> {
            self.sampler.sample(log)
        }

        fn flush(&self) -> SamplingBatch<LogSamplingContext> {
            let batch = self.sampler.flush();
            let resume_receiver = self.resume_receiver.lock().take();
            if let Some(resume_receiver) = resume_receiver {
                self.paused_sender.send(()).unwrap();
                resume_receiver.recv().unwrap();
            }
            batch
        }
    }

    let probe = LogProbe::default();
    let (paused_sender, paused_receiver) = mpsc::unbounded();
    let (resume_sender, resume_receiver) = mpsc::unbounded();
    let sampler = PausedFlush {
        sampler: SaveUntilFlush::new(&probe),
        paused_sender,
        resume_receiver: Mutex::<_>::new(Some(resume_receiver)),
    };
    let sink = builder()
        .log(probe.clone().with_sampler(sampler))
        .clock(SimpleClock::new_frozen())
        .build()
        .unwrap();
    emit!(sink, Message { value: 1 });

    thread::scope(|scope| {
        let flush = scope.spawn(|| sink.flush());
        paused_receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        emit!(sink, Message { value: 2 });
        resume_sender.send(()).unwrap();
        flush.join().unwrap().unwrap();
    });
    sink.flush().unwrap();

    assert_eq!(probe.attribute_values("value"), [Some(AnyValue::Int(1)), Some(AnyValue::Int(2))]);
}
