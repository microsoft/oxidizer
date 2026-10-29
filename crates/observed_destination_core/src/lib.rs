// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # Observed Destination Core
//!
//! This crate provides an integration layer between OpenTelemetry and Observed ecosystems.
//! The central concept is a "destination" that receives log records or metric data.
//! Each destination implements [`LogDestination`] or
//! [`MetricDestination`] (or both via separate types).
//!
//! This crate carries only the destination abstractions and the shared building
//! blocks that destinations are written against. The destinations themselves live
//! in companion crates such as `observed_destination_stdout`, so an application
//! depends on exactly the ones it uses — and can implement its own against the
//! same abstractions when none of them fits.
//!
//! [`FlushableMetricsProvider`] extends OpenTelemetry's meter-provider contract
//! with explicit flushing. A processor can retain a full
//! provider, including a raw-measurement forwarding provider, without depending
//! on a particular SDK implementation or deriving ownership from cached instruments.
//! Its flush method uses [`DestinationResult`] so implementations need not
//! expose an OpenTelemetry SDK's error type.
//!
//! # Examples
//!
//! These examples store exported data in memory. They use the `testing` feature
//! of `opentelemetry_sdk` for its in-memory exporters.
//!
//! ## Register a log destination
//!
//! A destination adds an exporter or processor to the provider builder.
//! This destination exports each log record as soon as the logger emits it.
//!
//! ```
//! use observed_destination_core::{DestinationResult, LogDestination, LoggerProviderBuilderExt};
//! use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
//! use opentelemetry_sdk::logs::{InMemoryLogExporter, LoggerProviderBuilder, SdkLoggerProvider};
//!
//! #[derive(Debug)]
//! struct MemoryLogs(InMemoryLogExporter);
//!
//! impl LogDestination for MemoryLogs {
//!     fn register(
//!         self,
//!         builder: LoggerProviderBuilder,
//!     ) -> DestinationResult<LoggerProviderBuilder> {
//!         Ok(builder.with_simple_exporter(self.0))
//!     }
//! }
//!
//! let exporter = InMemoryLogExporter::default();
//! let provider = SdkLoggerProvider::builder()
//!     .register(MemoryLogs(exporter.clone()))?
//!     .build();
//! let logger = provider.logger("example");
//! let mut record = logger.create_log_record();
//! record.set_body("request received".into());
//! logger.emit(record);
//!
//! assert_eq!(exporter.get_emitted_logs()?.len(), 1);
//! # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
//! ```
//!
//! ## Register and flush a metric destination
//!
//! This destination exports metrics at regular intervals.
//! The `record_and_flush` function accepts any [`FlushableMetricsProvider`].
//! It records a measurement and flushes it without depending on a specific SDK.
//!
//! ```
//! use observed_destination_core::{
//!     DestinationResult, FlushableMetricsProvider, MeterProviderBuilderExt, MetricDestination,
//! };
//! use opentelemetry_sdk::metrics::{
//!     InMemoryMetricExporter, MeterProviderBuilder, SdkMeterProvider,
//! };
//!
//! #[derive(Debug)]
//! struct MemoryMetrics(InMemoryMetricExporter);
//!
//! impl MetricDestination for MemoryMetrics {
//!     fn register(
//!         self,
//!         builder: MeterProviderBuilder,
//!     ) -> DestinationResult<MeterProviderBuilder> {
//!         Ok(builder.with_periodic_exporter(self.0))
//!     }
//! }
//!
//! fn record_and_flush(provider: &impl FlushableMetricsProvider) -> DestinationResult<()> {
//!     let counter = provider.meter("example").u64_counter("requests").build();
//!     counter.add(1, &[]);
//!     provider.force_flush()
//! }
//!
//! let exporter = InMemoryMetricExporter::default();
//! let provider = SdkMeterProvider::builder()
//!     .register(MemoryMetrics(exporter.clone()))?
//!     .build();
//! record_and_flush(&provider)?;
//!
//! assert!(!exporter.get_finished_metrics()?.is_empty());
//! # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
//! ```

pub use filter::{SeverityFilter, severity_of_level};
pub use format::{LogFormatConfig, Rfc3339Nanos, print_log_record};
pub use meter_provider::FlushableMetricsProvider;
pub use provider_ext::{LoggerProviderBuilderExt, MeterProviderBuilderExt};
pub use resource_header::ResourceHeader;

/// Result type for destination operations.
pub type DestinationResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A destination that receives log records.
///
/// Implementations add themselves (as exporters or processors) to an
/// `OTel` [`opentelemetry_sdk::logs::LoggerProviderBuilder`].
pub trait LogDestination: std::fmt::Debug {
    /// Adds this destination to a logger provider builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be registered with the builder.
    /// Specific errors depend on the destination implementation and may include
    /// configuration issues, conflicts, or other conditions.
    fn register(
        self,
        builder: opentelemetry_sdk::logs::LoggerProviderBuilder,
    ) -> DestinationResult<opentelemetry_sdk::logs::LoggerProviderBuilder>;
}

/// A destination that receives metric data.
///
/// Implementations add themselves (as exporters or readers) to an
/// `OTel` [`opentelemetry_sdk::metrics::MeterProviderBuilder`].
pub trait MetricDestination: std::fmt::Debug {
    /// Adds this destination to a meter provider builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be registered with the builder.
    /// Specific errors depend on the destination implementation and may include
    /// configuration issues, conflicts, or other conditions.
    fn register(
        self,
        builder: opentelemetry_sdk::metrics::MeterProviderBuilder,
    ) -> DestinationResult<opentelemetry_sdk::metrics::MeterProviderBuilder>;
}

/// Anything that yields a [`LogDestination`].
///
/// It can be a destination itself, or a `bon` builder for one, which is finished on the
/// caller's behalf.
///
/// Implementing this trait allows skipping the explicit `.build()` call on
/// a `bon` builder when passing it to a function that expects a `LogDestination`.
pub trait IntoLogDestination {
    /// The destination produced by [`into_log_destination`](Self::into_log_destination).
    type Destination: LogDestination + 'static;

    /// Produces the destination, finishing a builder when needed.
    fn into_log_destination(self) -> Self::Destination;
}

impl<D: LogDestination + 'static> IntoLogDestination for D {
    type Destination = Self;

    fn into_log_destination(self) -> Self {
        self
    }
}

/// Anything that yields a [`MetricDestination`].
///
/// It can be a destination itself, or a `bon` builder for one, which is finished on the
/// caller's behalf.
///
/// Implementing this trait allows skipping the explicit `.build()` call on
/// a `bon` builder when passing it to a function that expects a `MetricDestination`.
pub trait IntoMetricDestination {
    /// The destination produced by [`into_metric_destination`](Self::into_metric_destination).
    type Destination: MetricDestination + 'static;

    /// Produces the destination, finishing a builder when needed.
    fn into_metric_destination(self) -> Self::Destination;
}

impl<D: MetricDestination + 'static> IntoMetricDestination for D {
    type Destination = Self;

    fn into_metric_destination(self) -> Self {
        self
    }
}

mod filter;
mod format;
mod meter_provider;
mod provider_ext;
mod resource_header;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::{IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination};

    #[derive(Debug, PartialEq)]
    struct TestDestination {
        id: u32,
    }

    impl LogDestination for TestDestination {
        fn register(
            self,
            builder: opentelemetry_sdk::logs::LoggerProviderBuilder,
        ) -> crate::DestinationResult<opentelemetry_sdk::logs::LoggerProviderBuilder> {
            Ok(builder)
        }
    }

    impl MetricDestination for TestDestination {
        fn register(
            self,
            builder: opentelemetry_sdk::metrics::MeterProviderBuilder,
        ) -> crate::DestinationResult<opentelemetry_sdk::metrics::MeterProviderBuilder> {
            Ok(builder)
        }
    }

    #[test]
    fn log_destination_is_into_log_destination() {
        let test_destination = TestDestination { id: 1 };

        assert_eq!(test_destination.into_log_destination(), TestDestination { id: 1 });
    }

    #[test]
    fn metric_destination_is_into_metric_destination() {
        let test_destination = TestDestination { id: 2 };

        assert_eq!(test_destination.into_metric_destination(), TestDestination { id: 2 });
    }
}
