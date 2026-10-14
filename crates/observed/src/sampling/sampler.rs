// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The event sampler and its decision.

use crate::sampling::EventSamplingContext;

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
