// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_memory/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_destination_memory/favicon.ico"
)]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

//! In-memory log and metric destinations for testing.
//!
//! Use these destinations in tests that make sure that code emits the expected logs
//! and metrics. Each destination sends the data to an `opentelemetry_sdk` in-memory
//! exporter. Clones of an exporter share one buffer. Keep a clone and read the
//! captured data from it.
//!
//! Add this crate to `[dev-dependencies]`. It turns on the `testing` feature of
//! `opentelemetry_sdk`. Cargo merges the features that all crates in a build request
//! for a dependency. A normal dependency on this crate thus turns on `testing` for
//! every user of `opentelemetry_sdk` in that build.
//!
//! # Examples
//!
//! ```
//! use observed_destination_core::LoggerProviderBuilderExt;
//! use observed_destination_memory::memory_logs;
//! use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
//! use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
//!
//! let exporter = InMemoryLogExporter::default();
//! let provider = SdkLoggerProvider::builder()
//!     .register(memory_logs(exporter.clone()))?
//!     .build();
//!
//! // The code under test emits through a logger from the provider.
//! let logger = provider.logger("example");
//! let mut record = logger.create_log_record();
//! record.set_body("request received".into());
//! logger.emit(record);
//!
//! // The test reads the captured logs through its clone of the exporter.
//! assert_eq!(exporter.get_emitted_logs()?.len(), 1);
//! # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
//! ```
//!
//! For metrics, use [`memory_metrics`]. Metric values reach the exporter only when the
//! provider flushes or shuts down, or when the export interval ends.

use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Waker};
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
/// use observed_destination_core::LoggerProviderBuilderExt;
/// use observed_destination_memory::memory_logs;
/// use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
/// use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
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
/// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
/// ```
pub fn memory_logs(exporter: InMemoryLogExporter) -> MemoryLogDestinationBuilder<memory_log_destination_builder::SetExporter> {
    MemoryLogDestination::builder().exporter(exporter)
}

/// An in-memory log destination.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct MemoryLogDestination {
    /// Sets the in-memory exporter that receives the logs.
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
#[derive(derive_more::Debug)]
struct MemoryLogProcessor {
    // The shared buffer can hold many records, so `Debug` leaves the exporter out.
    #[debug(ignore)]
    exporter: InMemoryLogExporter,
    // This flag controls admission. An emit that already read `false` can
    // continue during shutdown and add a record after shutdown cleared the buffer.
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
        assert!(
            future.poll(&mut cx).is_ready(),
            "InMemoryLogExporter::export has no await points, so the first poll always completes"
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

/// Creates an in-memory metric destination backed by the given exporter.
///
/// Cloned exporters share the same buffer, so keep a clone to read the metrics.
/// The destination uses a periodic reader, which exports the values on a timer.
/// Recorded values reach the buffer only when the provider flushes, when it shuts
/// down, or when the export interval ends. The interval is 60 seconds by default.
/// To change it, set the `OTEL_METRIC_EXPORT_INTERVAL` environment variable in
/// milliseconds. Call `force_flush` on the provider before you read the metrics.
///
/// Shutdown exports the remaining values and keeps the buffer. You can read the
/// metrics after the provider shuts down.
///
/// # Examples
///
/// ```
/// use observed_destination_core::MeterProviderBuilderExt;
/// use observed_destination_memory::memory_metrics;
/// use opentelemetry::metrics::MeterProvider as _;
/// use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
///
/// let exporter = InMemoryMetricExporter::default();
///
/// let provider = SdkMeterProvider::builder()
///     .register(memory_metrics(exporter.clone()))?
///     .build();
///
/// let counter = provider.meter("example").u64_counter("requests").build();
/// counter.add(1, &[]);
///
/// // Flush to move the recorded values into the shared buffer.
/// provider.force_flush()?;
/// assert!(!exporter.get_finished_metrics()?.is_empty());
///
/// // Shutdown keeps the shared buffer.
/// provider.shutdown()?;
/// assert!(!exporter.get_finished_metrics()?.is_empty());
/// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
/// ```
pub fn memory_metrics(exporter: InMemoryMetricExporter) -> MemoryMetricDestinationBuilder<memory_metric_destination_builder::SetExporter> {
    MemoryMetricDestination::builder().exporter(exporter)
}

/// An in-memory metric destination.
#[derive(bon::Builder, Debug)]
#[builder(derive(Debug))]
pub struct MemoryMetricDestination {
    /// Sets the in-memory exporter that receives the metrics.
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

    use opentelemetry::logs::{LogRecord, Logger, LoggerProvider};
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

    #[test]
    fn processor_debug_omits_captured_records() {
        let processor = MemoryLogProcessor::new(InMemoryLogExporter::default());
        let provider = SdkLoggerProvider::builder().build();
        let logger = provider.logger("memory");
        let scope = InstrumentationScope::builder("memory").build();
        let mut record = logger.create_log_record();
        record.set_body("captured body".into());

        processor.emit(&mut record, &scope);

        // The shared buffer can grow to many records, so they must stay out of the `Debug` output.
        assert!(!format!("{processor:?}").contains("captured body"));
    }
}
