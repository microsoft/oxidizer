// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for [`Sink`].

use std::any::type_name;
use std::fmt;
use std::sync::Arc;

use tick::SimpleClock;

use crate::processing::EventProcessor;
use crate::sampling::EventSampler;
use crate::{Sink, SinkId};

/// Builds a [`Sink`].
///
/// Created by [`Sink::builder`], which takes the sink's id. All other settings
/// are optional:
///
/// | Setting | Default |
/// |---|---|
/// | [`processors`](Self::processors) | None. The sink drops every event. |
/// | [`clock`](Self::clock) | `SimpleClock::new_system()`. Timestamps come from the operating system. |
/// | [`isolated_enrichment`](Self::isolated_enrichment) | Off. The sink sees entries added with [`enrich`] and entries that [`enrich_for`] addresses to its id. |
/// | [`event_sampler`](Self::event_sampler) | None. Every event reaches the sink's interested processors. |
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
///
/// use observed::Sink;
/// # use observed::FlushError;
/// # use observed::metadata::EventDescription;
/// # use observed::processing::{EventProcessor, EventView};
/// # struct LogProcessor;
/// # impl EventProcessor for LogProcessor {
/// #     fn is_interested(&self, _description: &EventDescription) -> bool {
/// #         true
/// #     }
/// #     fn process(&self, _event: &EventView<'_>) {}
/// #     fn flush(&self) -> Result<(), FlushError> {
/// #         Ok(())
/// #     }
/// # }
///
/// let sink = Sink::builder("service")
///     .processors(vec![Arc::new(LogProcessor)])
///     .build();
/// ```
///
/// [`enrich`]: crate::enrichment::EnrichFutureExt::enrich
/// [`enrich_for`]: crate::enrichment::EnrichFutureExt::enrich_for
pub struct SinkBuilder {
    id: SinkId,
    clock: SimpleClock,
    processors: Vec<Arc<dyn EventProcessor>>,
    isolated_enrichment: bool,
    sampler: Option<Arc<dyn EventSampler>>,
}

impl SinkBuilder {
    pub(super) fn new(id: SinkId) -> Self {
        Self {
            id,
            clock: SimpleClock::new_system(),
            processors: Vec::new(),
            isolated_enrichment: false,
            sampler: None,
        }
    }

    /// Sets the [`EventProcessor`]s the sink fans each event out to,
    /// replacing any processors set earlier.
    ///
    /// Without processors the sink drops every event.
    #[must_use]
    pub fn processors(mut self, processors: Vec<Arc<dyn EventProcessor>>) -> Self {
        self.processors = processors;
        self
    }

    /// Sets the clock that stamps the timestamp on every event the sink
    /// dispatches, replacing the default system clock.
    ///
    /// Accepts anything that is `AsRef<SimpleClock>` - both
    /// [`tick::SimpleClock`] and [`tick::Clock`]. Pass the application's clock
    /// if it has one. In tests pass a frozen clock
    /// (`SimpleClock::new_frozen()`) for deterministic, Miri-safe timestamps.
    #[must_use]
    pub fn clock(mut self, clock: impl AsRef<SimpleClock>) -> Self {
        self.clock = clock.as_ref().clone();
        self
    }

    /// Makes the sink ignore entries added with
    /// [`enrich`](crate::enrichment::EnrichFutureExt::enrich): it sees only
    /// entries that [`enrich_for`](crate::enrichment::EnrichFutureExt::enrich_for)
    /// addresses to its id.
    ///
    /// Useful for library sinks that must not inherit application-level
    /// context.
    #[must_use]
    pub fn isolated_enrichment(mut self) -> Self {
        self.isolated_enrichment = true;
        self
    }

    /// Attaches one [`EventSampler`] to the sink, replacing any sampler set
    /// earlier.
    ///
    /// Takes the sampler itself or a `Box<dyn EventSampler>`, for example a
    /// sampler chosen at runtime. A boxed sampler must already have the type
    /// `Box<dyn EventSampler>`; a `Box<MySampler>` is not accepted, so pass
    /// `MySampler` itself instead.
    ///
    /// A sink that is not interested in an event never calls the sampler. See
    /// [`EventSampler::sample`] for the invocation and decision contract.
    ///
    /// The sampler is fixed once the sink is built. A
    /// [`composite`](Sink::composite) has no sampler of its own: each sink in
    /// it decides with the sampler it was built with. To share sampling state
    /// between sinks, see [`EventSampler`].
    ///
    /// # Examples
    ///
    /// A sampler chosen at runtime:
    ///
    /// ```
    /// # use observed::{EventSamplingContext, EventSamplingDecision};
    /// use observed::{EventSampler, Sink};
    /// # struct DropAll;
    /// # impl EventSampler for DropAll {
    /// #     fn sample(&self, _event: &EventSamplingContext<'_>) -> EventSamplingDecision {
    /// #         EventSamplingDecision::Drop
    /// #     }
    /// # }
    /// # struct KeepAll;
    /// # impl EventSampler for KeepAll {
    /// #     fn sample(&self, _event: &EventSamplingContext<'_>) -> EventSamplingDecision {
    /// #         EventSamplingDecision::Continue
    /// #     }
    /// # }
    ///
    /// let drop_all = false; // for example, read from configuration
    /// let sampler: Box<dyn EventSampler> = if drop_all {
    ///     Box::new(DropAll)
    /// } else {
    ///     Box::new(KeepAll)
    /// };
    /// let sink = Sink::builder("service").event_sampler(sampler).build();
    /// ```
    #[must_use]
    pub fn event_sampler(mut self, sampler: impl Into<Box<dyn EventSampler>>) -> Self {
        self.sampler = Some(Arc::from(sampler.into()));
        self
    }

    /// Builds the sink.
    #[must_use]
    pub fn build(self) -> Sink {
        Sink::new(self.id, self.processors, self.clock, self.isolated_enrichment, self.sampler)
    }
}

impl fmt::Debug for SinkBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("id", &self.id)
            .field("clock", &self.clock)
            .field("processors", &self.processors.len())
            .field("isolated_enrichment", &self.isolated_enrichment)
            .field("event_sampler", &self.sampler.is_some())
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::FlushError;
    use crate::metadata::EventDescription;
    use crate::processing::EventView;
    use crate::sampling::{EventSamplingContext, EventSamplingDecision};

    struct NoopProcessor;

    impl EventProcessor for NoopProcessor {
        fn is_interested(&self, _description: &EventDescription) -> bool {
            true
        }

        fn process(&self, _event: &EventView<'_>) {}

        fn flush(&self) -> Result<(), FlushError> {
            Ok(())
        }
    }

    struct KeepAll;

    impl EventSampler for KeepAll {
        fn sample(&self, _event: &EventSamplingContext<'_>) -> EventSamplingDecision {
            EventSamplingDecision::Continue
        }
    }

    #[test]
    fn default_builder_debug_reports_defaults() {
        let debug = format!("{:?}", Sink::builder("plain"));

        assert!(debug.contains("SinkBuilder"), "{debug}");
        assert!(debug.contains("\"plain\""), "{debug}");
        assert!(debug.contains("SimpleClock(System)"), "{debug}");
        assert!(debug.contains("processors: 0"), "{debug}");
        assert!(debug.contains("isolated_enrichment: false"), "{debug}");
        assert!(debug.contains("event_sampler: false"), "{debug}");
    }

    #[test]
    fn configured_builder_debug_reports_every_setting() {
        let builder = Sink::builder("configured")
            .processors(vec![Arc::new(NoopProcessor)])
            .clock(SimpleClock::new_frozen())
            .isolated_enrichment()
            .event_sampler(KeepAll);

        let debug = format!("{builder:?}");

        assert!(debug.contains("processors: 1"), "{debug}");
        assert!(debug.contains("SimpleClock(Controlled"), "{debug}");
        assert!(debug.contains("isolated_enrichment: true"), "{debug}");
        assert!(debug.contains("event_sampler: true"), "{debug}");
    }

    #[test]
    fn build_without_optional_settings_has_no_processors() {
        let sink = Sink::builder("plain").build();

        assert_eq!(sink.id(), SinkId::new("plain"));
        assert!(sink.is_noop());
    }

    #[test]
    fn build_keeps_the_configured_processors() {
        let sink = Sink::builder("processing").processors(vec![Arc::new(NoopProcessor)]).build();

        assert!(!sink.is_noop());
    }

    #[test]
    fn later_processors_replace_earlier_ones() {
        let sink = Sink::builder("replaced")
            .processors(vec![Arc::new(NoopProcessor)])
            .processors(Vec::new())
            .build();

        assert!(sink.is_noop());
    }
}
