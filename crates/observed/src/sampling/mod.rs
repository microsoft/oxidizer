// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event sampling for [`Sink`](crate::Sink)s.
//!
//! A Sink with an attached [`EventSampler`] asks it whether the event goes
//! ahead before asking its processors. The sampler receives an
//! [`EventSamplingContext`] with the event description, and can ask through it
//! whether any of the Sink's processors wants the event. Attach one with
//! [`Sink::with_event_sampler`](crate::Sink::with_event_sampler); a Sink without
//! one lets its processors decide.
//!
//! A sampler owns its own synchronization and must return its decision
//! synchronously.

use std::any::type_name;
use std::cell::OnceCell;
use std::fmt::{self, Debug, Formatter};

use crate::metadata::EventDescription;

/// Read-only inputs for one [`EventSampler`] decision.
pub struct EventSamplingContext<'a> {
    description: &'a EventDescription,
    get_first_interested_processor: &'a dyn Fn() -> Option<usize>,
    first_interested_processor: OnceCell<Option<usize>>,
}

impl<'a> EventSamplingContext<'a> {
    pub(crate) const fn new(description: &'a EventDescription, get_first_interested_processor: &'a dyn Fn() -> Option<usize>) -> Self {
        Self {
            description,
            get_first_interested_processor,
            first_interested_processor: OnceCell::new(),
        }
    }

    pub(crate) fn into_first_interested_processor(self) -> Option<usize> {
        self.first_interested_processor
            .into_inner()
            .unwrap_or_else(self.get_first_interested_processor)
    }

    /// Returns the event's type-level description.
    #[must_use]
    pub const fn description(&self) -> &EventDescription {
        self.description
    }

    /// Returns whether any of the Sink's processors wants the event.
    ///
    /// The Sink reuses these answers when it delivers the event, so each
    /// processor is asked at most once per event, however often this is called.
    #[must_use]
    pub fn processors_interested(&self) -> bool {
        self.first_interested_processor
            .get_or_init(self.get_first_interested_processor)
            .is_some()
    }
}

impl Debug for EventSamplingContext<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("description", self.description)
            .finish_non_exhaustive()
    }
}

/// What a Sink does with an event offered to its [`EventSampler`].
#[expect(
    clippy::exhaustive_enums,
    reason = "event sampling is a predicate with exactly two states, not an extensible workflow"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSamplingDecision {
    /// Continue to processor interest checks and deliver the event to those that want it.
    Continue,
    /// Run none of this Sink's processors for the event.
    Drop,
}

/// Decides whether an event reaches a Sink's processors.
///
/// Attach an implementation with
/// [`Sink::with_event_sampler`](crate::Sink::with_event_sampler). Each Sink
/// takes its own sampler; to share state between Sinks, keep the state behind
/// an [`Arc`](std::sync::Arc) and give each Sink a sampler holding a clone.
///
/// See the
/// [`event_sampling` example](https://github.com/microsoft/oxidizer/blob/main/crates/observed/examples/event_sampling.rs)
/// for a complete sampler and registration.
pub trait EventSampler: Send + Sync + 'static {
    /// Decides whether the event described by `context` reaches this Sink's
    /// processors.
    ///
    /// Called once for every event emitted through a Sink that has
    /// processors, before that Sink's processors are asked.
    /// [`EventSamplingDecision::Continue`] delivers the event to the processors
    /// that want it, and [`EventSamplingDecision::Drop`] runs none of them.
    /// Each processor is asked at most once: a sampler can ask through
    /// [`EventSamplingContext::processors_interested`], and the Sink asks them
    /// itself otherwise. The event is built once, when the first processor
    /// receives it, so an event no processor receives is never built.
    ///
    /// The call runs on the emitting thread. Emitting internal telemetry from
    /// here is not supported.
    fn sample(&self, context: &EventSamplingContext<'_>) -> EventSamplingDecision;
}

impl<T: EventSampler> From<T> for Box<dyn EventSampler> {
    fn from(sampler: T) -> Self {
        Box::new(sampler)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn description() -> EventDescription {
        EventDescription::new("probe.event", None, None, None, false, false)
    }

    #[test]
    fn processors_interested_asks_processors_once() {
        let asked = Cell::new(0);
        let get_first_interested_processor = || {
            asked.set(asked.get() + 1);
            Some(1)
        };
        let description = description();
        let context = EventSamplingContext::new(&description, &get_first_interested_processor);

        assert!(context.processors_interested());
        assert!(context.processors_interested());
        assert_eq!(context.into_first_interested_processor(), Some(1));
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn processors_interested_is_false_when_no_processor_wants_event() {
        let get_first_interested_processor = || None;
        let description = description();
        let context = EventSamplingContext::new(&description, &get_first_interested_processor);

        assert!(!context.processors_interested());
    }

    #[test]
    fn into_first_interested_processor_asks_processors_when_sampler_did_not() {
        let asked = Cell::new(0);
        let get_first_interested_processor = || {
            asked.set(asked.get() + 1);
            Some(0)
        };
        let description = description();
        let context = EventSamplingContext::new(&description, &get_first_interested_processor);

        assert_eq!(context.into_first_interested_processor(), Some(0));
        assert_eq!(asked.get(), 1);
    }

    #[test]
    fn debug_shows_description() {
        let description = description();
        let context = EventSamplingContext::new(&description, &|| None);

        let debug = format!("{context:?}");

        assert!(debug.contains("EventSamplingContext"));
        assert!(debug.contains("probe.event"));
    }
}
