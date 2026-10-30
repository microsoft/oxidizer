// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Stdout log and metric destinations.

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/CRATE_NAME/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/CRATE_NAME/favicon.ico"
)]
// Temporary compatibility: remove as this crate migrates to Anvil defaults.
#![allow(
    clippy::empty_structs_with_brackets,
    reason = "Temporary compatibility with existing empty structs during migration to Anvil defaults"
)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

use std::any::type_name;
use std::io::{Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use log::LevelFilter;
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{LogProcessor, LoggerProviderBuilder, SdkLogRecord};
use opentelemetry_sdk::metrics::MeterProviderBuilder;
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_stdout::MetricExporter;

use observed_destination_core::ResourceHeader;
use observed_destination_core::{
    DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination,
};
use observed_destination_core::{LogFormatConfig, print_log_record};
use observed_destination_core::{SeverityFilter, severity_of_level};

/// Creates a log destination that writes to stdout.
///
/// Optionally filters by severity level (defaults to `Info`).
///
/// # Examples
///
/// ```no_run
/// use observed_destination_stdout::stdout_logs;
/// use log::LevelFilter;
///
/// let dest = stdout_logs().level(LevelFilter::Debug).build();
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
        Ok(self.register_with_output(builder, || stdout().lock()))
    }
}

impl<S: stdout_log_destination_builder::IsComplete> IntoLogDestination
for StdoutLogDestinationBuilder<S>
{
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
struct StdoutLogProcessor<F> {
    resource_header: ResourceHeader,
    is_shutdown: AtomicBool,
    config: LogFormatConfig,
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

impl<F> std::fmt::Debug for StdoutLogProcessor<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

impl<F, W> LogProcessor for StdoutLogProcessor<F>
where
    F: Fn() -> W + Send + Sync + 'static,
    W: Write,
{
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        // SeqCst orders this check with the shutdown flag change.
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
        // SeqCst publishes shutdown in the same order observed by emit.
        self.is_shutdown.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource_header.set_resource(resource);
    }
}

// ─── Stdout metric destination ───────────────────────────────────────────

/// Creates a metric destination that writes to stdout.
pub fn stdout_metrics() -> StdoutMetricDestinationBuilder {
    StdoutMetricDestination::builder()
}

/// A metric destination that writes to stdout via the `OTel` stdout exporter.
#[derive(bon::Builder, Debug)]
pub struct StdoutMetricDestination {}

impl StdoutMetricDestination {
    fn register_with_exporter(
        builder: MeterProviderBuilder,
        exporter: impl PushMetricExporter,
    ) -> MeterProviderBuilder {
        builder.with_periodic_exporter(exporter)
    }
}

impl MetricDestination for StdoutMetricDestination {
    fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
        Ok(Self::register_with_exporter(
            builder,
            MetricExporter::default(),
        ))
    }
}

impl<S: stdout_metric_destination_builder::IsComplete> IntoMetricDestination
for StdoutMetricDestinationBuilder<S>
{
    type Destination = StdoutMetricDestination;

    fn into_metric_destination(self) -> StdoutMetricDestination {
        self.build()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use observed_destination_core::{LoggerProviderBuilderExt, MeterProviderBuilderExt};
    use opentelemetry::KeyValue;
    use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;
    use std::io::{Error, ErrorKind, Result as IoResult, sink};

    use super::*;

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
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn formats_processor_debug() {
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), sink);

        assert!(format!("{processor:?}").contains("StdoutLogProcessor"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn stdout_destination_registers_and_flushes() {
        let provider = SdkLoggerProvider::builder()
            .register(stdout_logs())
            .unwrap()
            .build();

        provider.force_flush().unwrap();
        provider.shutdown().unwrap();
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn log_destination_filters_records() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .level(LevelFilter::Warn)
            .build()
            .register_with_output(SdkLoggerProvider::builder(), output.clone().into_factory())
            .build();
        let logger = provider.logger("stdout");

        for (severity, body) in [
            (Severity::Info, "filtered-info"),
            (Severity::Warn, "visible-warning"),
        ] {
            let mut record = logger.create_log_record();
            record.set_severity_number(severity);
            record.set_body(body.into());
            logger.emit(record);
        }

        let written = output.contents();
        assert!(written.contains("visible-warning"), "{written}");
        assert!(!written.contains("filtered-info"), "{written}");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn log_destination_applies_format_configuration() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .format_config(LogFormatConfig {
                format_body: true,
                ..LogFormatConfig::default()
            })
            .build()
            .register_with_output(SdkLoggerProvider::builder(), output.clone().into_factory())
            .build();
        let logger = provider.logger("stdout");
        let mut record = logger.create_log_record();
        record.set_body("request {id}".into());
        record.add_attribute("id", 42);

        logger.emit(record);

        let written = output.contents();
        assert!(written.contains("request 42"), "{written}");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn log_destination_forwards_provider_resource() {
        let output = RecordingWriter::default();
        let provider = stdout_logs()
            .build()
            .register_with_output(
                SdkLoggerProvider::builder().with_resource(
                    Resource::builder_empty()
                        .with_attributes([KeyValue::new(
                            "service.name",
                            "stdout-contract-resource",
                        )])
                        .build(),
                ),
                output.clone().into_factory(),
            )
            .build();
        let logger = provider.logger("stdout");
        let record = logger.create_log_record();

        logger.emit(record);

        let written = output.contents();
        assert!(written.contains("stdout-contract-resource"), "{written}");
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn processor_shutdown_stops_exporting() {
        let output = RecordingWriter::default();
        let processor =
            StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());
        let scope = InstrumentationScope::builder("stdout").build();
        let mut record = SdkLoggerProvider::builder()
            .build()
            .logger("stdout")
            .create_log_record();
        record.set_body("before-shutdown".into());

        processor.emit(&mut record, &scope);

        let before_shutdown = output.contents();
        assert!(
            before_shutdown.contains("before-shutdown"),
            "{before_shutdown}"
        );

        processor
            .shutdown_with_timeout(Duration::from_secs(1))
            .unwrap();
        processor.emit(&mut record, &scope);

        assert_eq!(output.contents(), before_shutdown);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn force_flush_reaches_writer() {
        let output = RecordingWriter::default();
        let processor =
            StdoutLogProcessor::new(LogFormatConfig::default(), output.clone().into_factory());

        processor.force_flush().unwrap();

        assert_eq!(output.0.lock().flushes, 1);
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ResourceHeader::new calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn force_flush_reports_writer_error() {
        let output = RecordingWriter::default();
        output.0.lock().fail_flush = true;
        let processor = StdoutLogProcessor::new(LogFormatConfig::default(), output.into_factory());

        let error = processor.force_flush().unwrap_err();

        assert!(matches!(error, OTelSdkError::InternalFailure(_)));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel MeterProviderBuilder::build eagerly calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn stdout_metric_destination_registers_and_flushes() {
        let provider = SdkMeterProvider::builder()
            .register(stdout_metrics())
            .unwrap()
            .build();

        provider.force_flush().unwrap();
        provider.shutdown().unwrap();
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel MeterProviderBuilder::build eagerly calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn metric_destination_exports_recorded_metrics() {
        let exporter = InMemoryMetricExporter::default();
        let provider = StdoutMetricDestination::register_with_exporter(
            SdkMeterProvider::builder(),
            exporter.clone(),
        )
            .build();

        provider
            .meter("stdout")
            .u64_counter("stdout_contract_requests")
            .build()
            .add(2, &[]);
        provider.force_flush().unwrap();

        assert!(!exporter.get_finished_metrics().unwrap().is_empty());

        provider.shutdown().unwrap();
    }
}
