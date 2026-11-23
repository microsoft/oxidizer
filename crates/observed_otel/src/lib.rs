// Copyright (c) Microsoft Corporation.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
// TODO(doc-coverage): remove once `missing_docs` is promoted to [workspace.lints.rust].
#![deny(missing_docs)]

//! # Observed Otel - Sink Pipeline Builders
//!
//! This crate builds [`observed::Sink`] values and processor lists from log and metric destinations.
//! Destination implementations live in companion crates such as `observed_destination_stdout`.
//! Each sink gets its own `EventProcessor`s via the builder
//!
//! ## Quick start
//!
//! Use [`observed_otel::builder`](builder()) to attach destinations and
//! produce an [`observed::Sink`]:
//!
//! ```rust
//! # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! use observed_destination_stdout::{stdout_logs, stdout_metrics};
//!
//! let sink = observed_otel::builder()
//!     .log(stdout_logs())
//!     .metric(stdout_metrics())
//!     .build()?;
//! // Call `.id(ID)` to set a stable identity for
//! // `enrich_for(ID, …)` targeting.
//!
//! // ... run application, drain producers, then flush with `sink.flush()?`.
//! // Metric processor Drop also flushes best-effort before releasing ownership.
//! // Automatic log provider shutdown occurs after its last SDK handle is dropped.
//! # Ok(())
//! # }
//! ```
//!
//! To fan an event through multiple emitters in one `emit!`, build each
//! sink independently and compose them with [`observed::Sink::composite`].
//!
//! Each sink owns its own enrichment slot; entries pushed on one sink
//! are not automatically visible on another. See the "Limitations" section
//! of the API usability analysis for the tradeoff and workarounds.
//!
//! ## Custom meter providers
//!
//! An embedding host can supply a full OpenTelemetry meter provider implementing
//! [`observed_destination_core::FlushableMetricsProvider`] to
//! [`MetricEventProcessor::new`]. The same processor handles event
//! metadata, metric dimensions, enrichment, and redaction regardless of the
//! provider. This supports providers that forward raw measurements to
//! another runtime without introducing an SDK aggregation pipeline in Rust.
//!
//! The processor retains the provider independently of its instrument cache.
//! Sink flush delegates to the provider; processor Drop also flushes best-effort
//! before releasing its handle and cached instruments. This exports pending
//! metrics even if another provider clone is retained, without terminating a
//! pipeline that other consumers may still use. Final cleanup follows the
//! provider's own ownership and Drop semantics.
//!
//! Drain producers and explicitly flush while exporter/runtime resources are
//! still available to observe failures. Drop cannot report errors and cannot
//! cover measurements recorded after its flush. A forwarding provider controls
//! the sending side; the receiver must flush its own aggregation and exporters.
//!
//! ```
//! use data_privacy::RedactionEngine;
//! use observed::{emit, event};
//! use observed_otel::MetricEventProcessor;
//! use opentelemetry::InstrumentationScope;
//! use opentelemetry_sdk::metrics::SdkMeterProvider;
//!
//! #[event("requests")]
//! #[counter]
//! struct Request;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! // A forwarding provider can be supplied through the same contract.
//! let provider = SdkMeterProvider::builder().build();
//! let processor = MetricEventProcessor::new(
//!     provider,
//!     RedactionEngine::builder().build(),
//!     Some(InstrumentationScope::builder("hosted-library").build()),
//! );
//! let sink = observed_otel::builder().processor(processor).build()?;
//! emit!(sink, Request);
//! sink.flush()?; // Flushes the retained provider.
//! drop(sink); // Flushes best-effort and releases this sink's provider ownership.
//! # Ok(())
//! # }
//! ```
//!
//! ## Log sampling
//!
//! Log sampling lets your application choose which log records to send. For
//! example, you can keep error logs while sending fewer routine progress logs.
//!
//! Implement [`sampling::LogSampler`] and attach it per log destination with
//! [`LogDestinationExt::with_sampler`]. Your sampler receives enriched and
//! redacted log records.
//!
//! Return records in a [`sampling::SamplingBatch`] to send them to that
//! destination. Return an empty batch to send nothing. A sampler can also save
//! records and return them on a later call.
//!
//! Each log keeps the OpenTelemetry context captured before sampling; later
//! delivery uses that context rather than the caller's. See
//! [`sampling::LogSamplingContext`] for context access and lifetime details.
//!
//! [`Sink::flush`](observed::Sink::flush) sends the records that
//! [`sampling::LogSampler::flush`] returns. See that method for flush and drop rules.

mod builder;
mod destination_ext;
mod destination_overlay;
mod inst_scope_helpers;
mod instrument_cache;
mod log_event_processor;
mod metric_event_processor;
mod processor_builder;
mod redaction_policy;

pub mod sampling;

pub use builder::{Builder, builder};
pub use destination_ext::{LogDestinationExt, MetricDestinationExt};
pub use destination_overlay::DestinationOverlay;
pub use inst_scope_helpers::IntoInstrumentationScope;
pub use log_event_processor::LogEventProcessor;
pub use metric_event_processor::MetricEventProcessor;
pub use processor_builder::{
    ErasedLogDestination, ErasedMetricDestination, ProcessorBuilder, processor_builder,
};
pub use redaction_policy::RedactionPolicy;

#[cfg(test)]
mod test_support {
    use std::borrow::Cow;
    use std::ops::ControlFlow;

    use observed::event;
    use observed::interop::DynEvent;
    use observed::metadata::EventDescription;
    use observed::processing::FieldVisitorFn;

    #[event("test.log_only")]
    #[info]
    pub(crate) struct LogOnly;

    // Synthetic views require DynEvent; typed emission always supplies source metadata.
    pub(crate) struct UnsourcedEvent(pub(crate) EventDescription);

    impl DynEvent for UnsourcedEvent {
        fn name(&self) -> &'static str {
            self.0.name()
        }

        fn body(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn source_file(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn source_line(&self) -> Option<u32> {
            None
        }

        fn source_crate(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }

        fn description(&self) -> EventDescription {
            self.0
        }
    }

    #[event("test.request.count")]
    #[info]
    #[counter]
    pub(crate) struct TestRequestCount {
        #[dimension(metric = "status")]
        #[unredacted]
        pub(crate) status: i64,
    }
}
