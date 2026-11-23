// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Destination-aware builder for an [`observed::Sink`].
//!
//! [`Builder`] wraps a [`ProcessorBuilder`] and adds sink-level knobs
//! (`id`, `isolated_enrichment`, `event_sampler`). Use when the goal is a
//! ready-to-use [`Sink`]. Construct via [`builder`].
//!
//! Internally, [`Builder::build`] runs [`ProcessorBuilder::build`] to get
//! the processor list, then constructs the [`Sink`] via
//! [`Sink::builder`]. Each per-destination processor owns its `OTel` provider
//! and flushes it on demand. See each processor's documentation for shutdown.

use observed::processing::EventProcessor;
use observed::{EventSampler, Sink, SinkId};
use observed_destination_core::DestinationResult;
use opentelemetry_sdk::Resource;
use performables::arc::Arc;
use tick::SimpleClock;

use crate::{ErasedLogDestination, ErasedMetricDestination, ProcessorBuilder, RedactionPolicy, processor_builder};

/// Creates a destination-aware [`Builder`] for an [`observed::Sink`].
pub fn builder() -> Builder {
    Builder {
        pipeline: processor_builder(),
        id: None,
        isolated_enrichment: false,
        clock: None,
        event_sampler: None,
    }
}

/// Destination-aware builder for an [`observed::Sink`].
///
/// Wraps a [`ProcessorBuilder`] and adds sink-level knobs ([`id`](Self::id),
/// [`isolated_enrichment`](Self::isolated_enrichment), and
/// [`event_sampler`](Self::event_sampler)). All
/// pipeline-configuration methods (`log`, `metric`, `processor`, `processors`,
/// `redaction`, `resource`) are forwarded to the inner pipeline builder.
///
/// The sink's optional [`SinkId`] is an identity token used by
/// `enrich_for(ID, …)` to target enrichments.
///
/// Severity filtering is **per-destination** (via each destination's
/// `.level(...)` method) or **per-event** (via the `disabled` attribute in
/// `#[event("...", disabled)]`) — there is no sink-wide severity knob.
///
/// Configure the timestamp source via [`clock`](Self::clock) (optional;
/// defaults to the system clock when unset), then terminate with `build`,
/// which returns a [`DestinationResult<Sink>`].
///
/// Use [`observed::Sink::composite`] to fan one `emit!` out to multiple
/// emitters.
#[must_use]
pub struct Builder {
    pipeline: ProcessorBuilder,
    id: Option<SinkId>,
    isolated_enrichment: bool,
    clock: Option<SimpleClock>,
    event_sampler: Option<Box<dyn EventSampler>>,
}

impl std::fmt::Debug for Builder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("pipeline", &self.pipeline)
            .field("id", &self.id)
            .field("isolated_enrichment", &self.isolated_enrichment)
            .finish_non_exhaustive()
    }
}

impl Builder {
    /// Sets the sink's [`SinkId`] — the identity token used by
    /// `enrich_for(ID, …)` to target this sink.
    ///
    /// Accepts either an [`SinkId`] (typically a `static` for use with
    /// `enrich_for`) or a `&'static str` (string literal). When not called, the
    /// sink still has a placeholder id; targeted enrichment via
    /// `enrich_for` simply won't match it.
    pub fn id(mut self, id: impl Into<SinkId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Configures the assembled sink to ignore untargeted (global)
    /// enrichments. Targeted entries (via `enrich_for(ID, …)`) still reach it.
    pub fn isolated_enrichment(mut self) -> Self {
        self.isolated_enrichment = true;
        self
    }

    /// Adds a raw processor (forwards to
    /// [`ProcessorBuilder::processor`](ProcessorBuilder::processor)).
    pub fn processor(mut self, processor: impl EventProcessor + 'static) -> Self {
        self.pipeline = self.pipeline.processor(processor);
        self
    }

    /// Adds multiple raw processors (forwards to
    /// [`ProcessorBuilder::processors`](ProcessorBuilder::processors)).
    pub fn processors(mut self, processors: impl IntoIterator<Item = impl Into<Arc<dyn EventProcessor>>>) -> Self {
        self.pipeline = self.pipeline.processors(processors);
        self
    }

    /// Adds a log destination (forwards to
    /// [`ProcessorBuilder::log`](ProcessorBuilder::log)).
    pub fn log(mut self, destination: impl Into<ErasedLogDestination> + 'static) -> Self {
        self.pipeline = self.pipeline.log(destination.into());
        self
    }

    /// Adds a metric destination (forwards to
    /// [`ProcessorBuilder::metric`](ProcessorBuilder::metric)).
    pub fn metric(mut self, destination: impl Into<ErasedMetricDestination> + 'static) -> Self {
        self.pipeline = self.pipeline.metric(destination.into());
        self
    }

    /// Sets the sink-level baseline [`RedactionPolicy`] (forwards to
    /// [`ProcessorBuilder::redaction`](ProcessorBuilder::redaction)).
    pub fn redaction(mut self, redaction: RedactionPolicy) -> Self {
        self.pipeline = self.pipeline.redaction(redaction);
        self
    }

    /// Sets the `OTel` [`Resource`] (forwards to
    /// [`ProcessorBuilder::resource`](ProcessorBuilder::resource)).
    pub fn resource(mut self, resource: Resource) -> Self {
        self.pipeline = self.pipeline.resource(resource);
        self
    }

    /// Sets the clock the sink uses to stamp event timestamps.
    ///
    /// Optional: when not called, the sink uses the system clock
    /// ([`tick::SimpleClock::new_system`]). Accepts anything that is
    /// `AsRef<SimpleClock>` — both [`tick::SimpleClock`] and [`tick::Clock`].
    /// Pass a frozen clock (`SimpleClock::new_frozen()`) in tests for
    /// deterministic, Miri-safe timestamps.
    pub fn clock(mut self, clock: impl AsRef<SimpleClock>) -> Self {
        self.clock = Some(clock.as_ref().clone());
        self
    }

    /// Sets the [`EventSampler`] that decides which events reach the sink's
    /// processors (attached with [`observed::SinkBuilder::event_sampler`]).
    pub fn event_sampler(mut self, sampler: impl EventSampler) -> Self {
        self.event_sampler = Some(Box::new(sampler));
        self
    }

    /// Builds the processor list via [`ProcessorBuilder::build`] and
    /// constructs the [`Sink`]. If a [`clock`](Self::clock) was set it is
    /// used to stamp event timestamps; otherwise the system clock is used.
    /// Applies [`isolated_enrichment`](Self::isolated_enrichment) and
    /// [`event_sampler`](Self::event_sampler) to the [`Sink::builder`] when
    /// requested.
    ///
    /// # Errors
    ///
    /// Returns the destination's error if a log or metric destination fails to register.
    ///
    /// # Panics
    ///
    /// Panics if the assembled sink contains more than 64 processors.
    pub fn build(self) -> DestinationResult<Sink> {
        let Self {
            pipeline,
            id,
            isolated_enrichment,
            clock,
            event_sampler,
        } = self;
        let processors = pipeline.build()?.into_iter().map(Arc::into_std_arc).collect();
        let id = id.unwrap_or_else(|| SinkId::new("empty.service.name"));
        let clock = clock.unwrap_or_else(SimpleClock::new_system);
        let mut builder = Sink::builder(id).processors(processors).clock(clock);
        if isolated_enrichment {
            builder = builder.isolated_enrichment();
        }
        Ok(match event_sampler {
            Some(sampler) => builder.event_sampler(sampler).build(),
            None => builder.build(),
        })
    }
}

#[cfg(test)]
mod tests {
    use observed::FlushError;
    use observed::metadata::EventDescription;
    use observed::processing::{EventProcessor, EventView};
    use observed_destination_memory::{memory_logs, memory_metrics};
    use opentelemetry_sdk::logs::InMemoryLogExporter;
    use opentelemetry_sdk::metrics::InMemoryMetricExporter;

    struct NoopProcessor;

    impl EventProcessor for NoopProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            false
        }

        fn process(&self, _event: &EventView<'_>) {}

        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }

    /// Without `.id(...)`, the resulting sink falls back to the
    /// `observed::Sink::builder` placeholder id.
    #[test]
    fn build_without_id_uses_placeholder() {
        let sink = crate::builder()
            .log(memory_logs(InMemoryLogExporter::default()))
            .metric(memory_metrics(InMemoryMetricExporter::default()))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build");

        assert_eq!(sink.id().label(), "empty.service.name");
    }

    /// `.id(...)` propagates onto the resulting sink's id.
    #[test]
    fn id_propagates_to_emitter() {
        let sink = crate::builder()
            .id("custom")
            .log(memory_logs(InMemoryLogExporter::default()))
            .clock(tick::SimpleClock::new_frozen())
            .build()
            .expect("build");

        assert_eq!(sink.id().label(), "custom");
    }

    #[test]
    fn processor_limit_accepts_64_and_rejects_65() {
        let with_processors = |count| (0..count).fold(crate::builder(), |builder, _| builder.processor(NoopProcessor));

        with_processors(64).build().unwrap();
        assert!(
            std::panic::catch_unwind(|| with_processors(65).build()).is_err(),
            "a sink with more than 64 processors must be rejected"
        );
    }
}
