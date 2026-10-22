// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

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

mod filter;
mod format;
mod meter_provider;
mod provider_ext;
mod resource_header;

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

/// Anything that yields a [`LogDestination`] — a destination itself, or a
/// `bon` builder for one, which is finished on the caller's behalf.
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

/// Anything that yields a [`MetricDestination`] — a destination itself, or a
/// `bon` builder for one, which is finished on the caller's behalf.
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
