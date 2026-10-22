// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Houses simple inversion of control for registering log and metric destinations
//! with the otel logger and meter provider builders.

use opentelemetry_sdk::logs::LoggerProviderBuilder;
use opentelemetry_sdk::metrics::MeterProviderBuilder;

use crate::{DestinationResult, IntoLogDestination, IntoMetricDestination, LogDestination, MetricDestination};

/// Convenience extension trait to register log destinations with the otel logger provider builder.
///
/// # Examples
///
/// ```
/// use observed_destination_core::LoggerProviderBuilderExt;
/// use opentelemetry_sdk::logs::SdkLoggerProvider;
/// # use observed_destination_core::{DestinationResult, LogDestination};
/// # use opentelemetry_sdk::logs::LoggerProviderBuilder;
/// #
/// # #[derive(Debug)]
/// # struct MyLogDestination;
/// #
/// # impl LogDestination for MyLogDestination {
/// #     fn register(self, builder: LoggerProviderBuilder) -> DestinationResult<LoggerProviderBuilder> {
/// #         Ok(builder)
/// #     }
/// # }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// let provider = SdkLoggerProvider::builder()
///     .register(MyLogDestination)?
///     .build();
/// # Ok(())
/// # }
/// ```
pub trait LoggerProviderBuilderExt {
    /// Registers a log destination on this logger provider builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be registered with the builder.
    /// Specific errors depend on the destination implementation and may include
    /// configuration issues, conflicts, or other conditions.
    fn register(self, dest: impl IntoLogDestination) -> DestinationResult<LoggerProviderBuilder>;
}

impl LoggerProviderBuilderExt for LoggerProviderBuilder {
    fn register(self, dest: impl IntoLogDestination) -> DestinationResult<LoggerProviderBuilder> {
        dest.into_log_destination().register(self)
    }
}

/// Convenience extension trait to register metric destinations with the otel meter provider builder.
///
/// # Examples
///
/// ```
/// use observed_destination_core::MeterProviderBuilderExt;
/// use opentelemetry_sdk::metrics::SdkMeterProvider;
/// # use observed_destination_core::{DestinationResult, MetricDestination};
/// # use opentelemetry_sdk::metrics::MeterProviderBuilder;
/// #
/// # #[derive(Debug)]
/// # struct MyMetricDestination;
/// #
/// # impl MetricDestination for MyMetricDestination {
/// #     fn register(self, builder: MeterProviderBuilder) -> DestinationResult<MeterProviderBuilder> {
/// #         Ok(builder)
/// #     }
/// # }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
/// let provider = SdkMeterProvider::builder()
///     .register(MyMetricDestination)?
///     .build();
/// # Ok(())
/// # }
/// ```
pub trait MeterProviderBuilderExt {
    /// Registers a metric destination on this meter provider builder.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination cannot be registered with the builder.
    /// Specific errors depend on the destination implementation and may include
    /// configuration issues, conflicts, or other conditions.
    fn register(self, dest: impl IntoMetricDestination) -> DestinationResult<MeterProviderBuilder>;
}

impl MeterProviderBuilderExt for MeterProviderBuilder {
    fn register(self, dest: impl IntoMetricDestination) -> DestinationResult<MeterProviderBuilder> {
        dest.into_metric_destination().register(self)
    }
}
