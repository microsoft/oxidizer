// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_stdout/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_stdout/favicon.ico"
)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

//! In-memory log and metric destinations for testing.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use log::LevelFilter;
use observed_destination_core::{
    DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination, SeverityFilter, severity_of_level,
};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{InMemoryLogExporter, LogBatch, LogExporter, LogProcessor, LoggerProviderBuilder, SdkLogRecord};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, MeterProviderBuilder};

/// Creates an in-memory log destination backed by the given exporter.
///
/// Cloned exporters share the same buffer, so keep a clone to read the logs.
/// Read them before the [`SdkLoggerProvider`](opentelemetry_sdk::logs::SdkLoggerProvider)
/// shuts down. Shutdown clears the buffer. Dropping the last provider or logger
/// handle also shuts the provider down.
///
/// # Examples
///
/// ```
/// # use observed_destination_core::LoggerProviderBuilderExt;
/// # use observed_destination_memory::memory_logs;
/// # use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
/// # use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
///
/// let exporter = InMemoryLogExporter::default();
///
/// let provider = SdkLoggerProvider::builder()
///     .register(memory_logs(exporter.clone()))?
///     .build();
///
/// let logger = provider.logger("example");
/// let mut record = logger.create_log_record();
/// record.set_body("request received".into());
/// logger.emit(record);
///
/// // Read the logs through the shared exporter while the provider is alive.
/// assert_eq!(exporter.get_emitted_logs()?.len(), 1);
///
/// // Shutdown clears the shared buffer.
/// provider.shutdown()?;
/// assert!(exporter.get_emitted_logs()?.is_empty());
///
/// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
/// ```
pub fn memory_logs(exporter: InMemoryLogExporter) -> MemoryLogDestinationBuilder<memory_log_destination_builder::SetExporter> {
    MemoryLogDestination::builder().exporter(exporter)
}

/// An in-memory log destination.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct MemoryLogDestination {
    /// In-memory log exporter.
    exporter: InMemoryLogExporter,

    /// Sets the minimum log level (default: `Trace` - captures everything).
    #[builder(default = LevelFilter::Trace)]
    level: LevelFilter,
}

impl LogDestination for MemoryLogDestination {
    fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
        let processor = SeverityFilter::new(severity_of_level(self.level), MemoryLogProcessor::new(self.exporter));
        Ok(builder.with_log_processor(processor))
    }
}

impl<S: memory_log_destination_builder::IsComplete> IntoLogDestination for MemoryLogDestinationBuilder<S> {
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
#[derive(Debug)]
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

impl LogProcessor for MemoryLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, instrumentation: &InstrumentationScope) {
        if self.is_shutdown.load(Ordering::SeqCst) {
            return;
        }

        let batch = [(data as &SdkLogRecord, instrumentation)];
        let future = pin!(self.exporter.export(LogBatch::new(&batch)));
        let mut cx = Context::from_waker(Waker::noop());
        let ready = matches!(future.poll(&mut cx), Poll::Ready(_));
        assert!(ready, "InMemoryLogExporter::export must complete synchronously");
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

/// Creates an in-memory metric destination backed by the given exporter.
pub fn memory_metrics(exporter: InMemoryMetricExporter) -> MemoryMetricDestinationBuilder<memory_metric_destination_builder::SetExporter> {
    MemoryMetricDestination::builder().exporter(exporter)
}

/// An in-memory metric destination.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct MemoryMetricDestination {
    /// In-memory metric exporter.
    exporter: InMemoryMetricExporter,
}

impl MetricDestination for MemoryMetricDestination {
    fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
        Ok(builder.with_periodic_exporter(self.exporter))
    }
}

impl<S: memory_metric_destination_builder::IsComplete> IntoMetricDestination for MemoryMetricDestinationBuilder<S> {
    type Destination = MemoryMetricDestination;

    fn into_metric_destination(self) -> MemoryMetricDestination {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use opentelemetry::logs::{Logger, LoggerProvider};
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    use super::*;

    static_assertions::assert_impl_all!(MemoryLogDestination: UnwindSafe, RefUnwindSafe, Send, Sync);
    static_assertions::assert_impl_all!(MemoryLogDestinationBuilder: UnwindSafe, RefUnwindSafe, Send, Sync);

    static_assertions::assert_impl_all!(MemoryMetricDestination: UnwindSafe, RefUnwindSafe, Send, Sync);
    static_assertions::assert_impl_all!(MemoryMetricDestinationBuilder: UnwindSafe, RefUnwindSafe, Send, Sync);

    #[test]
    fn processor_shutdown_stops_exporting() {
        let exporter = InMemoryLogExporter::default();
        let processor = MemoryLogProcessor::new(exporter.clone());
        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("memory");
        let scope = InstrumentationScope::builder("memory").build();
        let mut record = logger.create_log_record();

        processor.emit(&mut record, &scope);
        assert_eq!(exporter.get_emitted_logs().unwrap().len(), 1);

        processor.shutdown_with_timeout(Duration::from_secs(1)).unwrap();
        assert!(exporter.is_shutdown_called());

        processor.emit(&mut record, &scope);
        assert!(exporter.get_emitted_logs().unwrap().is_empty());
    }
}
