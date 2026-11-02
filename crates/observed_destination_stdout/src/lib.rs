// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! # Observed stdout Destination
//!
//! Provides implementations of [`LogDestination`] and [`MetricDestination`] that write to stdout.
//!
//! # Examples
//!
//! ```
//! use log::LevelFilter;
//! use observed_destination_core::{LogDestination, LoggerProviderBuilderExt};
//! use observed_destination_stdout::stdout_logs;
//! use opentelemetry::logs::LoggerProvider;
//! use opentelemetry_sdk::logs::SdkLoggerProvider;
//!
//! let provider = SdkLoggerProvider::builder()
//!     .register(stdout_logs().level(LevelFilter::Warn))
//!     .unwrap()
//!     .build();
//!
//! let logger = provider.logger("stdout");
//! ```
//!
//! ```
//! use observed_destination_core::MeterProviderBuilderExt;
//! use opentelemetry::metrics::MeterProvider;
//! use opentelemetry_sdk::metrics::SdkMeterProvider;
//!
//! let provider = SdkMeterProvider::builder()
//!     .register(observed_destination_stdout::stdout_metrics())
//!     .unwrap()
//!     .build();
//!
//! provider
//!     .meter("stdout")
//!     .u64_counter("stdout_contract_requests")
//!     .build()
//!     .add(2, &[]);
//!
//! provider.shutdown();
//! ```

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_stdout/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_stdout/favicon.ico"
)]
// Temporary compatibility: remove as this crate migrates to Anvil defaults.
#![allow(
    clippy::empty_structs_with_brackets,
    reason = "Temporary compatibility with existing empty structs during migration to Anvil defaults"
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::fmt::Debug;
use std::io::{Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use log::LevelFilter;
use observed_destination_core::{
    DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, LogFormatConfig, MetricDestination, ResourceHeader,
    SeverityFilter, print_log_record, severity_of_level,
};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{LogProcessor, LoggerProviderBuilder, SdkLogRecord};
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_stdout::MetricExporter;

/// Creates a log destination that writes to stdout.
///
/// Optionally filters by severity level (defaults to `Info`). Formatting and writing occur
/// synchronously on the emitting thread which locks the stdout buffer. The destination holds the
/// stdout lock while writing, so a slow write can block that thread and other stdout writers.
///
/// # Examples
///
/// ```
/// use log::LevelFilter;
/// use observed_destination_core::{LogDestination, LoggerProviderBuilderExt};
/// use observed_destination_stdout::stdout_logs;
/// use opentelemetry::logs::LoggerProvider;
/// use opentelemetry_sdk::logs::SdkLoggerProvider;
///
/// let provider = SdkLoggerProvider::builder()
///     .register(stdout_logs().level(LevelFilter::Warn))
///     .unwrap()
///     .build();
///
/// let logger = provider.logger("stdout");
/// ```
pub fn stdout_logs() -> StdoutLogDestinationBuilder {
    StdoutLogDestination::builder()
}

/// A log destination that writes records to stdout.
#[derive(bon::Builder, Debug)]
pub struct StdoutLogDestination {
    /// Sets the minimum log level (default: `Info`).
    #[builder(default = LevelFilter::Info)]
    level: LevelFilter,

    /// Sets the log format configuration controlling which fields are included
    /// in the output (default: [`LogFormatConfig::default()`]).
    #[builder(default = LogFormatConfig::default())]
    format_config: LogFormatConfig,
}

impl StdoutLogDestination {
    fn register_with_output<W: Write>(
        self,
        builder: LoggerProviderBuilder,
        make_writer: impl Fn() -> W + Send + Sync + 'static,
    ) -> LoggerProviderBuilder {
        let processor = SeverityFilter::new(
            severity_of_level(self.level),
            StdoutLogProcessor::new(self.format_config, make_writer),
        );
        builder.with_log_processor(processor)
    }
}

impl LogDestination for StdoutLogDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        #[cfg_attr(coverage_nightly, coverage(off))]
        fn stdout_writer() -> std::io::StdoutLock<'static> {
            stdout().lock()
        }

        Ok(self.register_with_output(builder, stdout_writer))
    }
}

impl<S: stdout_log_destination_builder::IsComplete> IntoLogDestination for StdoutLogDestinationBuilder<S> {
    type Destination = StdoutLogDestination;

    fn into_log_destination(self) -> StdoutLogDestination {
        self.build()
    }
}

/// A [`LogProcessor`] that writes records straight to stdout from `emit`.
///
/// Stdout writes are synchronous, so there is no need to wrap an async
/// exporter in `block_on` (which panics inside another executor). Records are
/// formatted and printed inline.
#[derive(derive_more::Debug)]
struct StdoutLogProcessor<F> {
    resource_header: ResourceHeader,
    // This flag controls admission. An emit that has already observed false can
    // continue during shutdown. The flag only records shutdown state; it does not
    // wait for in-flight writes.
    is_shutdown: AtomicBool,
    config: LogFormatConfig,
    #[debug(ignore)]
    make_writer: F,
}

impl<F> StdoutLogProcessor<F> {
    fn new(config: LogFormatConfig, make_writer: F) -> Self {
        Self {
            resource_header: ResourceHeader::new(),
            is_shutdown: AtomicBool::new(false),
            config,
            make_writer,
        }
    }
}

impl<F, W> LogProcessor for StdoutLogProcessor<F>
where
    F: Fn() -> W + Send + Sync + 'static,
    W: Write,
{
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        if self.is_shutdown.load(Ordering::SeqCst) {
            return;
        }

        let mut w = (self.make_writer)();
        let _ = self.resource_header.print_once(&mut w);
        let _ = print_log_record(&mut w, data, instrumentation, self.config);
    }

    fn force_flush(&self) -> OTelSdkResult {
        (self.make_writer)()
            .flush()
            .map_err(|e| OTelSdkError::InternalFailure(e.to_string()))
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        self.is_shutdown.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource_header.set_resource(resource);
    }
}

// ─── Stdout metric destination ───────────────────────────────────────────

/// Creates a metric destination that writes to stdout.
///
/// The underlying implementation is the `opentelemetry_stdout` crate's
/// `MetricExporter`, which is intended for debugging and development purposes,
/// with an output format that can change.
///
/// Metrics are exported periodically, every 60 seconds by default. Set
/// `OTEL_METRIC_EXPORT_INTERVAL` to configure the interval. Recording a
/// measurement does not write it immediately. Call `force_flush` to export
/// pending measurements before exit; `shutdown` flushes pending measurements
/// and stops the periodic reader.
///
/// A stdout write error panics the thread performing the export, including
/// the periodic export worker.
///
/// # Examples
///
/// ```
/// use observed_destination_core::MeterProviderBuilderExt;
/// use opentelemetry::metrics::MeterProvider;
/// use opentelemetry_sdk::metrics::SdkMeterProvider;
///
/// let provider = SdkMeterProvider::builder()
///     .register(observed_destination_stdout::stdout_metrics())
///     .unwrap()
///     .build();
///
/// provider
///     .meter("stdout")
///     .u64_counter("stdout_contract_requests")
///     .build()
///     .add(2, &[]);
///
/// provider.force_flush().unwrap();
/// provider.shutdown().unwrap();
/// ```
pub fn stdout_metrics() -> StdoutMetricDestinationBuilder {
    StdoutMetricDestination::builder()
}

/// A metric destination that writes to stdout via the `OTel` stdout exporter.
#[derive(bon::Builder, Debug)]
pub struct StdoutMetricDestination {}

impl StdoutMetricDestination {
    fn register_with_exporter(builder: MeterProviderBuilder, exporter: impl PushMetricExporter) -> MeterProviderBuilder {
        builder.with_periodic_exporter(exporter)
    }
}

impl MetricDestination for StdoutMetricDestination {
    fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
        Ok(Self::register_with_exporter(builder, MetricExporter::default()))
    }
}

impl<S: stdout_metric_destination_builder::IsComplete> IntoMetricDestination for StdoutMetricDestinationBuilder<S> {
    type Destination = StdoutMetricDestination;

    fn into_metric_destination(self) -> StdoutMetricDestination {
        self.build()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::{Error, ErrorKind, Result as IoResult, sink};
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::mpsc;
    use std::thread;
    use std::time::SystemTime;

    use opentelemetry::KeyValue;
    use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;
    use rstest::rstest;

    use super::*;

    // 2026-10-30T09:42:47Z
    const TEST_TIMESTAMP_OFFSET: Duration = Duration::from_secs(1_793_353_367);

    static_assertions::assert_impl_all!(StdoutLogDestination: UnwindSafe, RefUnwindSafe, Send, Sync);
    static_assertions::assert_impl_all!(StdoutMetricDestination: UnwindSafe, RefUnwindSafe, Send, Sync);

    #[derive(Clone, Default)]
    struct RecordingWriter(Arc<Mutex<WriterState>>);

    impl RecordingWriter {
        fn into_factory(self) -> impl Fn() -> Self {
            move || self.clone()
        }

        fn contents(&self) -> String {
            String::from_utf8(self.0.lock().bytes.clone()).unwrap()
        }
    }

    #[derive(Default)]
    struct WriterState {
        bytes: Vec<u8>,
        flushes: usize,
        fail_flush: bool,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            self.0.lock().bytes.write(buf)
        }

        fn flush(&mut self) -> IoResult<()> {
            let mut state = self.0.lock();
            state.flushes += 1;
            if state.fail_flush {
                Err(Error::from(ErrorKind::BrokenPipe))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn formats_processor_debug() {
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), sink);

        assert!(format!("{processor:?}").contains("StdoutLogProcessor"));
    }

    #[test]
    #[expect(
        clippy::write_with_newline,
        reason = "the newline is part of the contract and should be kept explicit"
    )]
    fn log_destination_filters_records() {
        use std::fmt::Write;

        for (level_filter, outcomes) in [
            // Ensures that the default level filter is Info
            (
                None,
                &[
                    (Severity::Debug, "debug", false),
                    (Severity::Info, "info", true),
                    (Severity::Warn, "warn", true),
                    (Severity::Error, "error", true),
                    (Severity::Fatal, "fatal", true),
                ],
            ),
            // Ensures that the level filter is applied correctly
            (
                Some(LevelFilter::Error),
                &[
                    (Severity::Debug, "debug", false),
                    (Severity::Info, "info", false),
                    (Severity::Warn, "warn", false),
                    (Severity::Error, "error", true),
                    (Severity::Fatal, "fatal", true),
                ],
            ),
        ] {
            let output = RecordingWriter::default();

            let destination = match level_filter {
                Some(level) => stdout_logs().level(level).build(),
                None => stdout_logs().build(),
            };

            let provider = destination
                .register_with_output(SdkLoggerProvider::builder(), output.clone().into_factory())
                .with_resource(Resource::builder_empty().build())
                .build();
            let logger = provider.logger("stdout");

            let mut expected = "<Resource: >\n".to_string();

            for (severity, body, should_emit) in outcomes {
                let mut record = logger.create_log_record();
                record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
                record.set_severity_number(*severity);
                record.set_body((*body).into());
                logger.emit(record);

                if *should_emit {
                    write!(
                        expected,
                        "[2026-10-30T09:42:47.000000000Z] {}: {} [stdout-?]\n",
                        severity.name(),
                        *body
                    )
                    .unwrap();
                }
            }

            let written = output.contents();
            assert_eq!(written, expected, "Failed for level_filter={level_filter:?}");
        }
    }

    #[test]
    fn log_destination_applies_format_configuration() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .format_config(LogFormatConfig {
                format_body: true,
                ..LogFormatConfig::default()
            })
            .build()
            .register_with_output(SdkLoggerProvider::builder(), output.clone().into_factory())
            .with_resource(Resource::builder_empty().build())
            .build();

        let logger = provider.logger("stdout");
        let mut record = logger.create_log_record();
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        record.set_body("request {id}".into());
        record.add_attribute("id", 42);

        logger.emit(record);

        let written = output.contents();
        assert_eq!(written, "<Resource: >\n[2026-10-30T09:42:47.000000000Z] request 42 [stdout-?]\n");
    }

    #[test]
    fn log_destination_forwards_provider_resource() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .build()
            .register_with_output(
                SdkLoggerProvider::builder().with_resource(
                    Resource::builder_empty()
                        .with_attributes([KeyValue::new("service.name", "stdout-contract-resource")])
                        .build(),
                ),
                output.clone().into_factory(),
            )
            .build();

        let logger = provider.logger("stdout");

        let mut record = logger.create_log_record();
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        logger.emit(record);

        // Also emit a second record to verify that the resource header is only printed once.
        let mut record = logger.create_log_record();
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);
        logger.emit(record);

        let written = output.contents();
        assert_eq!(
            written,
            "<Resource:  service.name=stdout-contract-resource>\n[2026-10-30T09:42:47.000000000Z]  [stdout-?]\n[2026-10-30T09:42:47.000000000Z]  [stdout-?]\n"
        );
    }

    #[test]
    fn processor_shutdown_stops_exporting() {
        let output = RecordingWriter::default();
        let mut processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());
        processor.set_resource(&Resource::builder_empty().build());

        let scope = InstrumentationScope::builder("stdout").build();
        let mut record = SdkLoggerProvider::builder().build().logger("stdout").create_log_record();
        record.set_body("before-shutdown".into());
        record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);

        processor.emit(&mut record, &scope);

        let before_shutdown = output.contents();
        assert_eq!(
            before_shutdown,
            "<Resource: >\n[2026-10-30T09:42:47.000000000Z] before-shutdown [stdout-?]\n"
        );

        processor.shutdown_with_timeout(Duration::from_secs(1)).unwrap();
        processor.emit(&mut record, &scope);

        assert_eq!(output.contents(), before_shutdown);
    }

    #[test]
    fn started_emits_finish_after_shutdown() {
        let (emitter_started_tx, emitter_started_rx) = mpsc::channel::<()>();
        let (emitter_continue_tx, emitter_continue_rx) = mpsc::channel::<()>();
        let emitter_continue_rx = std::sync::Mutex::new(emitter_continue_rx);

        let output = RecordingWriter::default();
        let output_clone = output.clone();
        let mut processor = StdoutLogProcessor::new(LogFormatConfig::default(), move || {
            emitter_started_tx.send(()).unwrap();
            emitter_continue_rx.lock().unwrap().recv().unwrap();
            output_clone.clone()
        });
        processor.set_resource(&Resource::builder_empty().build());

        thread::scope(|s| {
            let t = s.spawn(|| {
                let scope = InstrumentationScope::builder("stdout").build();

                let mut record = SdkLoggerProvider::builder().build().logger("stdout").create_log_record();
                record.set_body("before-shutdown".into());
                record.set_timestamp(SystemTime::UNIX_EPOCH + TEST_TIMESTAMP_OFFSET);

                processor.emit(&mut record, &scope);
            });

            emitter_started_rx.recv().unwrap();

            processor.shutdown_with_timeout(Duration::from_secs(1)).unwrap();

            emitter_continue_tx.send(()).unwrap();

            t.join().unwrap();
        });

        assert_eq!(
            output.contents(),
            "<Resource: >\n[2026-10-30T09:42:47.000000000Z] before-shutdown [stdout-?]\n"
        );
    }

    #[test]
    fn force_flush_reaches_writer() {
        let output = RecordingWriter::default();
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());

        processor.force_flush().unwrap();

        assert_eq!(output.0.lock().flushes, 1);
    }

    #[test]
    fn force_flush_reports_writer_error() {
        let output = RecordingWriter::default();
        output.0.lock().fail_flush = true;
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.into_factory());

        let error = processor.force_flush().unwrap_err();

        let OTelSdkError::InternalFailure(error_message) = &error else {
            panic!("expected InternalFailure, got {error:?}")
        };

        assert_eq!(error_message, "broken pipe");
    }

    #[rstest]
    // Flushing must export pending measurements.
    #[case::force_flush(SdkMeterProvider::force_flush)]
    // Shutdown must export pending measurements without a prior flush.
    #[case::shutdown(SdkMeterProvider::shutdown)]
    fn metric_destination_exports_recorded_metrics(#[case] export: fn(&SdkMeterProvider) -> OTelSdkResult) {
        let exporter = InMemoryMetricExporter::default();
        let provider = StdoutMetricDestination::register_with_exporter(
            SdkMeterProvider::builder().with_resource(Resource::builder_empty().with_service_name("test service name").build()),
            exporter.clone(),
        )
        .build();

        provider.meter("stdout").u64_counter("stdout_contract_requests").build().add(2, &[]);

        export(&provider).unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();

        assert_eq!(metrics.len(), 1);

        let service_name = metrics[0].resource().get(&opentelemetry::Key::from_static_str("service.name"));
        let Some(opentelemetry::Value::String(service_name)) = service_name else {
            panic!("service.name attribute should be a string")
        };
        assert_eq!(service_name.as_str(), "test service name");

        let scope_metrics = &metrics[0].scope_metrics().next().unwrap();
        assert_eq!(scope_metrics.scope().name(), "stdout");

        let metric = scope_metrics.metrics().next().unwrap();
        assert_eq!(metric.name(), "stdout_contract_requests");

        let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
            panic!("expected a u64 counter sum");
        };

        let mut points = sum.data_points();
        assert_eq!(points.next().unwrap().value(), 2);
        assert!(points.next().is_none());
    }
}
