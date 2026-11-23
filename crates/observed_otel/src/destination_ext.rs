// Copyright (c) Microsoft Corporation.

//! Extension traits that attach per-destination overlays.
//!
//! [`LogDestinationExt`] and [`MetricDestinationExt`] are blanket-implemented
//! for every destination, so any destination can be wrapped into a
//! [`DestinationOverlay`] by calling an overlay method on it.

use crate::destination_overlay::DestinationOverlay;
use crate::inst_scope_helpers::IntoInstrumentationScope;
use crate::redaction_policy::RedactionPolicy;
use crate::sampling::LogSampler;

/// An extension trait for providing overlays and producing [`DestinationOverlay`]
pub trait LogDestinationExt: observed_destination_core::IntoLogDestination + Sized {
    /// Attaches a per-destination `Resource` overlay, returning a wrapper
    /// that carries the overlay alongside the destination.
    #[must_use]
    fn extend_resource(self, resource: opentelemetry_sdk::Resource) -> DestinationOverlay<Self>
    where
        Self: 'static,
    {
        DestinationOverlay {
            inner: self,
            resource: Some(resource),
            redaction: None,
            scope: None,
            sampler: None,
        }
    }

    /// Attaches a per-destination [`RedactionPolicy`] overlay, returning a
    /// wrapper that carries the overlay alongside the destination. The
    /// overlay is merged on top of the sink-level baseline at build
    /// time; overlay entries win on `DataClass` conflict.
    #[must_use]
    fn extend_redaction(self, redaction: RedactionPolicy) -> DestinationOverlay<Self>
    where
        Self: 'static,
    {
        DestinationOverlay {
            inner: self,
            resource: None,
            redaction: Some(redaction),
            scope: None,
            sampler: None,
        }
    }

    /// Attaches one log sampler to this destination.
    ///
    /// The sampler receives enriched and redacted records before the first
    /// `OTel` processor. Returned records are sent to this destination.
    /// Its decisions affect neither other destinations nor metrics. Calling
    /// [`DestinationOverlay::with_sampler`] again replaces the sampler; samplers
    /// are never chained.
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    /// use observed_destination_stdout::stdout_logs;
    /// use observed_otel::{LogDestinationExt, builder};
    /// use observed_otel::sampling::{LogSamplingContext, LogSampler, SamplingBatch};
    ///
    /// struct PassThrough;
    ///
    /// impl LogSampler for PassThrough {
    ///     fn sample(&self, log: LogSamplingContext) -> SamplingBatch<LogSamplingContext> {
    ///         SamplingBatch::one(log)
    ///     }
    ///
    ///     fn flush(&self) -> SamplingBatch<LogSamplingContext> {
    ///         SamplingBatch::empty()
    ///     }
    /// }
    ///
    /// let sink = builder()
    ///     .log(stdout_logs().with_sampler(PassThrough))
    ///     .build()?;
    /// sink.flush()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    fn with_sampler(self, sampler: impl LogSampler) -> DestinationOverlay<Self>
    where
        Self: 'static,
    {
        DestinationOverlay {
            inner: self,
            resource: None,
            redaction: None,
            scope: None,
            sampler: Some(Box::new(sampler)),
        }
    }
}

impl<D: observed_destination_core::IntoLogDestination> LogDestinationExt for D {}

/// An extension trait for providing overlays and producing [`DestinationOverlay`]
pub trait MetricDestinationExt: observed_destination_core::IntoMetricDestination + Sized {
    /// Attaches a per-destination `Resource` overlay, returning a wrapper
    /// that carries the overlay alongside the destination.
    #[must_use]
    fn extend_resource(self, resource: opentelemetry_sdk::Resource) -> DestinationOverlay<Self>
    where
        Self: 'static,
    {
        DestinationOverlay {
            inner: self,
            resource: Some(resource),
            redaction: None,
            scope: None,
            sampler: None,
        }
    }

    /// Attaches a per-destination [`RedactionPolicy`] overlay, returning a
    /// wrapper that carries the overlay alongside the destination. The
    /// overlay is merged on top of the sink-level baseline at build
    /// time; overlay entries win on `DataClass` conflict.
    #[must_use]
    fn extend_redaction(self, redaction: RedactionPolicy) -> DestinationOverlay<Self>
    where
        Self: 'static,
    {
        DestinationOverlay {
            inner: self,
            resource: None,
            redaction: Some(redaction),
            scope: None,
            sampler: None,
        }
    }

    /// Attaches an `OTel` [`opentelemetry::InstrumentationScope`] to this
    /// metric destination. The scope is forwarded to
    /// [`opentelemetry::metrics::MeterProvider::meter_with_scope`] when
    /// the meter is constructed at [`Builder::build`](crate::Builder::build) time,
    /// so every metric exported through this destination carries it.
    ///
    /// Accepts anything implementing [`IntoInstrumentationScope`]: a full
    /// [`opentelemetry::InstrumentationScope`] (with version / `schema_url` /
    /// attributes), or a string-like value (`&'static str`, `String`,
    /// `Cow<'static, str>`) for the name-only case.
    #[must_use]
    fn with_scope(self, scope: impl IntoInstrumentationScope) -> DestinationOverlay<Self>
    where
        Self: 'static,
    {
        DestinationOverlay {
            inner: self,
            resource: None,
            redaction: None,
            scope: Some(scope.into_instrumentation_scope()),
            sampler: None,
        }
    }
}

impl<D: observed_destination_core::IntoMetricDestination> MetricDestinationExt for D {}
