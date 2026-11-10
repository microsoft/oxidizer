// Copyright (c) Microsoft Corporation.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

//! In-memory log and metric destinations for testing.

use std::any::type_name;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use log::LevelFilter;
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{
    InMemoryLogExporter, LogBatch, LogExporter, LogProcessor, LoggerProviderBuilder, SdkLogRecord,
};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, MeterProviderBuilder};

use observed_destination_core::{
    DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination,
};
use observed_destination_core::{SeverityFilter, severity_of_level};

/// Creates an in-memory log destination backed by the given exporter.
///
/// Cloned exporters share the same buffer, so the caller can inspect collected
/// logs after the pipeline shuts down.
///
/// # Examples
///
/// ```no_run
/// use observed_destination_memory::memory_logs;
/// use opentelemetry_sdk::logs::InMemoryLogExporter;
///
/// let exporter = InMemoryLogExporter::default();
/// let dest = memory_logs(exporter.clone());
/// // ... after emission ...
/// let logs = exporter.get_emitted_logs().unwrap();
/// ```
pub fn memory_logs(
    exporter: InMemoryLogExporter,
) -> MemoryLogDestinationBuilder<memory_log_destination_builder::SetExporter> {
    MemoryLogDestination::builder().exporter(exporter)
}

/// An in-memory log destination.
#[derive(bon::Builder, Debug)]
pub struct MemoryLogDestination {
    /// In-memory log exporter.
    exporter: InMemoryLogExporter,

    /// Sets the minimum log level (default: `Trace` - captures everything).
    #[builder(default = LevelFilter::Trace)]
    level: LevelFilter,
}

impl LogDestination for MemoryLogDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        let processor = SeverityFilter::new(
            severity_of_level(self.level),
            MemoryLogProcessor::new(self.exporter),
        );
        Ok(builder.with_log_processor(processor))
    }
}

impl<S: memory_log_destination_builder::IsComplete> IntoLogDestination
    for MemoryLogDestinationBuilder<S>
{
    type Destination = MemoryLogDestination;

    fn into_log_destination(self) -> MemoryLogDestination {
        self.build()
    }
}

/// A [`LogProcessor`] that records into an [`InMemoryLogExporter`] from `emit`.
///
/// The in-memory exporter completes synchronously, so its export future is
/// driven to completion with a single poll instead of `block_on`, which would
/// panic inside another executor.
struct MemoryLogProcessor {
    exporter: InMemoryLogExporter,
    is_shutdown: AtomicBool,
}

impl MemoryLogProcessor {
    fn new(exporter: InMemoryLogExporter) -> Self {
        Self {
            exporter,
            is_shutdown: AtomicBool::new(false),
        }
    }
}

impl std::fmt::Debug for MemoryLogProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

impl LogProcessor for MemoryLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        if self.is_shutdown.load(Ordering::SeqCst) {
            return;
        }

        let batch = [(data as &SdkLogRecord, instrumentation)];
        let future = pin!(self.exporter.export(LogBatch::new(&batch)));
        let mut cx = Context::from_waker(Waker::noop());
        let ready = matches!(future.poll(&mut cx), Poll::Ready(_));
        assert!(
            ready,
            "InMemoryLogExporter::export must complete synchronously"
        );
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        self.is_shutdown.store(true, Ordering::SeqCst);
        self.exporter.shutdown()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.exporter.set_resource(resource);
    }
}

// ─── In-memory metric destination ────────────────────────────────────────

/// Creates an in-memory metric destination backed by the given exporter.
pub fn memory_metrics(
    exporter: InMemoryMetricExporter,
) -> MemoryMetricDestinationBuilder<memory_metric_destination_builder::SetExporter> {
    MemoryMetricDestination::builder().exporter(exporter)
}

/// An in-memory metric destination.
#[derive(bon::Builder, Debug)]
pub struct MemoryMetricDestination {
    /// In-memory metric exporter.
    exporter: InMemoryMetricExporter,
}

impl MetricDestination for MemoryMetricDestination {
    fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
        Ok(builder.with_periodic_exporter(self.exporter))
    }
}

impl<S: memory_metric_destination_builder::IsComplete> IntoMetricDestination
    for MemoryMetricDestinationBuilder<S>
{
    type Destination = MemoryMetricDestination;

    fn into_metric_destination(self) -> MemoryMetricDestination {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use observed_destination_core::LoggerProviderBuilderExt;
    use opentelemetry::logs::{Logger, LoggerProvider};
    use opentelemetry::{Key, KeyValue};
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use super::*;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel InMemoryLogExporter construction calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn formats_processor_debug() {
        let processor = MemoryLogProcessor::new(InMemoryLogExporter::default());

        assert!(format!("{processor:?}").contains("MemoryLogProcessor"));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel InMemoryLogExporter construction calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn exported_logs_carry_the_provider_resource() {
        let exporter = InMemoryLogExporter::default();
        let resource = Resource::builder_empty()
            .with_attributes([KeyValue::new("service.name", "memory-test")])
            .build();
        let provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .register(memory_logs(exporter.clone()))
            .unwrap()
            .build();
        let logger = provider.logger("memory");
        let record = logger.create_log_record();

        logger.emit(record);
        provider.force_flush().unwrap();

        let logs = exporter.get_emitted_logs().unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(
            logs[0].resource.get(&Key::from_static_str("service.name")),
            Some("memory-test".into()),
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel InMemoryLogExporter construction calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn processor_shutdown_stops_exporting() {
        let exporter = InMemoryLogExporter::default();
        let processor = MemoryLogProcessor::new(exporter.clone());
        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("memory");
        let scope = InstrumentationScope::builder("memory").build();
        let mut record = logger.create_log_record();

        processor.emit(&mut record, &scope);

        assert_eq!(exporter.get_emitted_logs().unwrap().len(), 1);

        processor
            .shutdown_with_timeout(Duration::from_secs(1))
            .unwrap();

        assert!(exporter.is_shutdown_called());

        processor.emit(&mut record, &scope);

        assert!(exporter.get_emitted_logs().unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "OTel InMemoryLogExporter construction calls current_exe, unsupported under Miri isolation"
    )]
    // TODO: Re-enable under Miri after upgrading opentelemetry_sdk to a release containing
    // https://github.com/open-telemetry/opentelemetry-rust/pull/3525.
    fn sdk_provider_shutdown_reaches_exporter() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .register(memory_logs(exporter.clone()))
            .unwrap()
            .build();

        provider.shutdown().unwrap();

        assert!(exporter.is_shutdown_called());
    }
}
