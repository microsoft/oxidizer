// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::ControlFlow;

use observed::FlushError;
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use opentelemetry::Context;
use opentelemetry::logs::{Logger, LoggerProvider};
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};

use crate::sampling::{LogSampler, LogSamplingContext, SamplingBatch};

const DEFAULT_LOGGER_NAME: &str = "observed_logger";

/// An [`EventProcessor`] that dispatches events as log records through `OTel`.
///
/// Use [`builder`](crate::builder()) or [`processor_builder`](crate::processor_builder())
/// to construct it from log destinations.
///
/// All log events are accepted; per-destination severity filtering happens
/// inside each destination's `OTel` processor via the destination's
/// `.level(...)` configuration, and library authors opt individual events out
/// via the `disabled` attribute on `#[event("...", disabled)]`.
///
/// This processor creates enriched and redacted log records from events.
/// If a sampler is supplied, the processor sends the records returned by that
/// sampler. Without a sampler, it sends each record directly to OpenTelemetry.
/// See [`LogSampler::flush`] for when the processor sends records that a sampler saves.
///
/// Dropping this processor does not call [`SdkLoggerProvider::shutdown`].
/// Other logger or provider handles remain usable. OpenTelemetry shuts down
/// the provider when its last logger or provider handle is dropped.
///
/// Captured contexts can keep spans and application-owned values alive.
pub struct LogEventProcessor {
    provider: SdkLoggerProvider,
    logger: SdkLogger,
    redaction_engine: data_privacy::RedactionEngine,
    sampler: Option<Box<dyn LogSampler>>,
}

impl LogEventProcessor {
    /// Creates a log processor with an optional destination-specific sampler.
    ///
    /// Pass `None` to send logs through this provider without sampling.
    #[must_use]
    pub(crate) fn new(
        provider: SdkLoggerProvider,
        redaction_engine: data_privacy::RedactionEngine,
        sampler: Option<Box<dyn LogSampler>>,
    ) -> Self {
        let logger = provider.logger(DEFAULT_LOGGER_NAME);
        Self {
            provider,
            logger,
            redaction_engine,
            sampler,
        }
    }

    fn emit_batch(&self, batch: SamplingBatch<LogSamplingContext>) {
        for LogSamplingContext { record, context } in batch {
            let _context_guard = context.attach();
            self.logger.emit(record);
        }
    }
}

impl Drop for LogEventProcessor {
    fn drop(&mut self) {
        // Release the sampler before this processor's logger and provider handles.
        drop(self.sampler.take());
    }
}

impl std::fmt::Debug for LogEventProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogEventProcessor").finish_non_exhaustive()
    }
}

impl EventProcessor for LogEventProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        !description.is_disabled()
            && description.log().is_some_and(|log| {
                self.logger
                    .event_enabled(observed_utils::otel_severity_of(log.severity()), "", Some(log.name()))
            })
    }

    fn process(&self, event: &EventView<'_>) {
        use opentelemetry::logs::{AnyValue, LogRecord};

        // Keep severity and the log-specific name from the same metadata snapshot.
        let description = event.description();
        let Some(log) = description.log() else {
            return;
        };
        let severity = log.severity();

        let mut record = self.logger.create_log_record();
        record.set_event_name(log.name());
        record.set_severity_number(observed_utils::otel_severity_of(severity));
        record.set_severity_text(severity.as_str());
        record.set_timestamp(event.timestamp());

        if let Some(body) = event.body() {
            record.set_body(AnyValue::String(body.into()));
        }

        let engine = &self.redaction_engine;

        // Visit event fields, skipping those excluded from logs.
        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value = observed_utils::any_value_of(value);
                record.add_attribute(opentelemetry::Key::from_static_str(log.key()), any_value);
            }
            ControlFlow::Continue(())
        });

        // Visit enrichments, skipping those excluded from logs.
        let _ = event.visit_enrichments(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value = observed_utils::any_value_of(value);
                record.add_attribute(opentelemetry::Key::from_static_str(log.key()), any_value);
            }
            ControlFlow::Continue(())
        });

        if let Some(file) = event.source_file() {
            record.add_attribute(KEY_CODE_FILE_PATH.clone(), AnyValue::String(file.into()));
        }
        if let Some(line) = event.source_line() {
            record.add_attribute(KEY_CODE_LINE_NUMBER.clone(), AnyValue::Int(i64::from(line)));
        }
        if let Some(crate_name) = event.source_crate() {
            record.add_attribute(KEY_CODE_NAMESPACE.clone(), AnyValue::String(crate_name.into()));
        }

        if let Some(sampler) = &self.sampler {
            let sampling_context = LogSamplingContext::new(record, Context::current());
            self.emit_batch(sampler.sample(sampling_context));
        } else {
            self.logger.emit(record);
        }
    }

    fn flush(&self) -> Result<(), FlushError> {
        if let Some(sampler) = &self.sampler {
            self.emit_batch(sampler.flush());
        }
        self.provider.force_flush().map_err(|e| FlushError::new("otel-logs", e))
    }
}

/// Pre-computed `OTel` key for source file path attribute.
static KEY_CODE_FILE_PATH: opentelemetry::Key = opentelemetry::Key::from_static_str("code.file.path");

/// Pre-computed `OTel` key for source line number attribute.
static KEY_CODE_LINE_NUMBER: opentelemetry::Key = opentelemetry::Key::from_static_str("code.line.number");

/// Pre-computed `OTel` key for source crate name attribute.
static KEY_CODE_NAMESPACE: opentelemetry::Key = opentelemetry::Key::from_static_str("code.namespace");

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error;
    use std::ops::ControlFlow;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use data_privacy::RedactionEngine;
    use observed::enrichment::EnrichFnExt;
    use observed::interop::{DynEvent, emit_dyn_event};
    use observed::metadata::{EventDescription, FieldDescriptor, LogDescription};
    use observed::processing::{EventProcessor, EventView, FieldVisitorFn};
    use observed::{Enrichment, Event, Severity, Sink, SinkId, Value, emit, event};
    use observed_destination_core::{SeverityFilter, severity_of_level};
    use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLogRecord, SdkLoggerProvider, SimpleLogProcessor};

    use crate::LogEventProcessor;
    use crate::test_support::{LogOnly, UnsourcedEvent};

    #[event("test.metric_only")]
    #[counter]
    struct MetricOnly;

    const ALLOCATION_KEY: &str = "allocation_test_field";
    const ALLOCATION_ENRICHMENT_KEY: &str = "allocation_test_enrichment";
    const ALLOCATION_BODY: &str = "allocation test body";
    const ALLOCATION_FILE: &str = "allocation_test_file.rs";
    const ALLOCATION_NAMESPACE: &str = "allocation_test_namespace";

    struct AllocationTestEvent {
        severity: Severity,
        owned: bool,
        conversions: AtomicUsize,
    }

    impl AllocationTestEvent {
        fn text(&self, text: &'static str) -> Cow<'static, str> {
            if self.owned {
                Cow::Owned(text.to_owned())
            } else {
                Cow::Borrowed(text)
            }
        }
    }

    impl DynEvent for AllocationTestEvent {
        fn name(&self) -> &'static str {
            "allocation-test"
        }

        fn body(&self) -> Option<Cow<'static, str>> {
            self.conversions.fetch_add(1, Ordering::Relaxed);
            Some(self.text(ALLOCATION_BODY))
        }

        fn source_file(&self) -> Option<Cow<'static, str>> {
            Some(self.text(ALLOCATION_FILE))
        }

        fn source_line(&self) -> Option<u32> {
            Some(42)
        }

        fn source_crate(&self) -> Option<Cow<'static, str>> {
            Some(self.text(ALLOCATION_NAMESPACE))
        }

        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            visitor(&FieldDescriptor::log_only(ALLOCATION_KEY), &|_| Value::from(7_i64))
        }

        fn description(&self) -> EventDescription {
            EventDescription::new(
                self.name(),
                None,
                Some(LogDescription::new(self.name(), self.severity, None)),
                None,
                false,
                false,
            )
        }
    }

    #[derive(Enrichment)]
    struct AllocationTestEnrichment {
        #[unredacted]
        #[dimension(log = "allocation_test_enrichment")]
        value: i64,
    }

    fn allocation_test_processor(exporter: InMemoryLogExporter, level: log::LevelFilter) -> LogEventProcessor {
        let provider = SdkLoggerProvider::builder()
            .with_log_processor(SeverityFilter::new(severity_of_level(level), SimpleLogProcessor::new(exporter)))
            .build();
        LogEventProcessor::new(provider, RedactionEngine::builder().build(), None)
    }

    fn attribute<'a>(record: &'a SdkLogRecord, key: &str) -> &'a AnyValue {
        &record.attributes_iter().find(|(name, _)| name.as_str() == key).unwrap().1
    }

    fn string(value: &AnyValue) -> &str {
        let AnyValue::String(value) = value else {
            panic!("expected string attribute");
        };
        value.as_str()
    }

    #[test]
    fn log_processor_ignores_events_without_a_log_signal() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();
        let processor = LogEventProcessor::new(provider, RedactionEngine::builder().build(), None);

        // Bypass Sink's interest check to reach the processor's missing-log branch.
        processor.process(&EventView::new_synthetic(
            &UnsourcedEvent(MetricOnly::DESCRIPTION),
            SystemTime::UNIX_EPOCH,
        ));

        assert!(exporter.get_emitted_logs().unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock to set the log's observed timestamp. This test runs natively and under cargo careful."
    )]
    fn log_processor_omits_absent_source_metadata() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();
        let processor = LogEventProcessor::new(provider, RedactionEngine::builder().build(), None);

        processor.process(&EventView::new_synthetic(
            &UnsourcedEvent(LogOnly::DESCRIPTION),
            SystemTime::UNIX_EPOCH,
        ));

        let logs = exporter.get_emitted_logs().unwrap();
        assert_eq!(logs[0].record.attributes_iter().count(), 0);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock to set the log's observed timestamp. This test runs natively and under cargo careful."
    )]
    fn borrowed_log_metadata_and_keys_are_not_copied() {
        for owned in [false, true] {
            let exporter = InMemoryLogExporter::default();
            let sink = Sink::builder(SinkId::new("metadata-copy-test"))
                .processors(vec![Arc::new(allocation_test_processor(exporter.clone(), log::LevelFilter::Info))])
                .clock(tick::SimpleClock::new_frozen())
                .build();
            let event = AllocationTestEvent {
                severity: Severity::Info,
                owned,
                conversions: AtomicUsize::new(0),
            };

            (|| emit_dyn_event(&sink, &event)).enrich(&sink, AllocationTestEnrichment { value: 11 })();
            sink.flush().unwrap();

            let logs = exporter.get_emitted_logs().unwrap();
            assert_eq!(logs.len(), 1);
            let record = &logs[0].record;
            assert_eq!(record.event_name(), Some("allocation-test"));
            assert_eq!(record.severity_number(), Some(opentelemetry::logs::Severity::Info));
            assert_eq!(attribute(record, ALLOCATION_KEY), &AnyValue::Int(7));
            assert_eq!(attribute(record, ALLOCATION_ENRICHMENT_KEY), &AnyValue::Int(11));
            assert_eq!(attribute(record, "code.line.number"), &AnyValue::Int(42));
            for expected in [ALLOCATION_KEY, ALLOCATION_ENRICHMENT_KEY] {
                let key = &record.attributes_iter().find(|(key, _)| key.as_str() == expected).unwrap().0;
                assert_eq!(key.as_str().as_ptr(), expected.as_ptr());
            }
            for (actual, expected) in [
                (string(record.body().unwrap()), ALLOCATION_BODY),
                (string(attribute(record, "code.file.path")), ALLOCATION_FILE),
                (string(attribute(record, "code.namespace")), ALLOCATION_NAMESPACE),
            ] {
                assert_eq!(actual, expected);
                if !owned {
                    assert_eq!(actual.as_ptr(), expected.as_ptr(), "static metadata was copied");
                }
            }
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock to set the log's observed timestamp. This test runs natively and under cargo careful."
    )]
    fn severity_filter_skips_conversion_independently_for_each_destination() {
        let info_exporter = InMemoryLogExporter::default();
        let error_exporter = InMemoryLogExporter::default();
        let off_exporter = InMemoryLogExporter::default();
        let sink = Sink::builder(SinkId::new("destination-filter-test"))
            .processors(vec![
                Arc::new(allocation_test_processor(info_exporter.clone(), log::LevelFilter::Info)),
                Arc::new(allocation_test_processor(error_exporter.clone(), log::LevelFilter::Error)),
                Arc::new(allocation_test_processor(off_exporter.clone(), log::LevelFilter::Off)),
            ])
            .clock(tick::SimpleClock::new_frozen())
            .build();

        for (severity, expected_conversions) in [
            (Severity::Trace, 0),
            (Severity::Debug, 0),
            (Severity::Info, 1),
            (Severity::Warn, 1),
            (Severity::Error, 2),
        ] {
            let event = AllocationTestEvent {
                severity,
                owned: false,
                conversions: AtomicUsize::new(0),
            };
            emit_dyn_event(&sink, &event);
            assert_eq!(event.conversions.load(Ordering::Relaxed), expected_conversions, "{severity:?}");
        }

        sink.flush().unwrap();
        assert_eq!(info_exporter.get_emitted_logs().unwrap().len(), 3);
        assert_eq!(error_exporter.get_emitted_logs().unwrap().len(), 1);
        assert!(off_exporter.get_emitted_logs().unwrap().is_empty());
    }

    #[test]
    fn log_processor_drop_preserves_retained_provider() {
        let provider = SdkLoggerProvider::builder().build();
        let processor = LogEventProcessor::new(provider.clone(), RedactionEngine::builder().build(), None);

        drop(processor);

        provider.shutdown().unwrap();
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OpenTelemetry reads the wall clock to set the log's observed timestamp. This test runs natively and under cargo careful."
    )]
    fn sdk_handles_keep_provider_usable_after_processor_drop() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_resource(Resource::builder_empty().build())
            .with_simple_exporter(exporter.clone())
            .build();
        let processor = LogEventProcessor::new(provider.clone(), RedactionEngine::builder().build(), None);

        drop(processor);

        let logger = provider.logger("sampling.external");
        drop(provider);
        let mut record = logger.create_log_record();
        record.set_event_name("sampling.external");
        record.set_body("after processor drop".into());
        logger.emit(record);

        {
            let logs = exporter.get_emitted_logs().unwrap();
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].record.event_name(), Some("sampling.external"));
            assert_eq!(logs[0].record.body(), Some(&AnyValue::String("after processor drop".into())));
        }
        assert!(!exporter.is_shutdown_called());

        drop(logger);

        assert!(exporter.is_shutdown_called());
    }

    #[test]
    fn log_processor_flush_preserves_provider_failures() {
        let provider = SdkLoggerProvider::builder()
            .with_batch_exporter(InMemoryLogExporter::default())
            .build();
        let processor = LogEventProcessor::new(provider.clone(), RedactionEngine::builder().build(), None);
        assert!(format!("{processor:?}").contains("LogEventProcessor"));

        // Stopping the worker makes subsequent flushes fail without a custom exporter.
        provider.shutdown().unwrap();

        let error = processor.flush().unwrap_err();

        assert_eq!(error.processor(), "otel-logs");
        assert!(error.source().is_some());
    }

    #[test]
    fn event_with_low_severity_not_reaches_logging_provider() {
        #[derive(Debug)]
        struct Probe {
            invoked: performables::arc::Arc<performables::sync::mutex::Mutex<bool>>,
            min_severity: opentelemetry::logs::Severity,
        }

        impl Clone for Probe {
            fn clone(&self) -> Self {
                Self {
                    invoked: performables::arc::Arc::clone(&self.invoked),
                    min_severity: self.min_severity,
                }
            }
        }

        impl observed_destination_core::LogDestination for Probe {
            fn register(
                self,
                builder: opentelemetry_sdk::logs::LoggerProviderBuilder,
            ) -> observed_destination_core::DestinationResult<opentelemetry_sdk::logs::LoggerProviderBuilder> {
                Ok(builder.with_log_processor(self))
            }
        }

        impl opentelemetry_sdk::logs::LogProcessor for Probe {
            fn emit(&self, _data: &mut opentelemetry_sdk::logs::SdkLogRecord, _instrumentation: &opentelemetry::InstrumentationScope) {
                *self.invoked.lock() = true;
            }

            fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
                Ok(())
            }

            fn event_enabled(&self, level: opentelemetry::logs::Severity, _target: &str, _name: Option<&str>) -> bool {
                level.ge(&self.min_severity)
            }
        }

        #[event("test.low_severity_log")]
        #[info]
        struct LogEvent {
            #[unredacted]
            value: i64,
        }

        static ID: SinkId = SinkId::new("low_severity_log_test");

        let fake_exporter_destination = Probe {
            invoked: performables::arc::Arc::new(performables::sync::mutex::Mutex::<_>::new(false)),
            min_severity: opentelemetry::logs::Severity::Error,
        };

        let sink = crate::builder()
            .id(ID)
            .log(fake_exporter_destination.clone())
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        emit!(sink, LogEvent { value: 1 });

        assert!(
            !*fake_exporter_destination.invoked.lock(),
            "severities lower than expected by the LogProcessor should be filtered out by EventProcessor's is_interested"
        );
    }

    #[test]
    fn disabled_event_is_not_emitted_as_log() {
        use opentelemetry_sdk::logs::InMemoryLogExporter;

        #[event("test.disabled_log", disabled)]
        #[info]
        struct DisabledLogEvent {
            #[unredacted]
            value: i64,
        }

        static ID: SinkId = SinkId::new("disabled_log_test");

        let exporter = InMemoryLogExporter::default();
        let exporter_clone = exporter.clone();
        let sink = crate::builder()
            .id(ID)
            .log(observed_destination_memory::memory_logs(exporter))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build should succeed");

        emit!(sink, DisabledLogEvent { value: 1 });

        sink.flush().expect("flush should succeed");

        let logs = exporter_clone.get_emitted_logs().expect("should get logs");
        assert!(logs.is_empty(), "disabled event should not produce logs");
    }
}
