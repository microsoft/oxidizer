// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event sampling for [`Sink`](crate::Sink)s.
//!
//! An [`EventSampler`] decides whether an event reaches a Sink's processors.
//! The sampler receives an [`EventSamplingContext`] with the event description,
//! Sink identity, and timestamp. Attach one with
//! [`Sink::with_event_sampler`](crate::Sink::with_event_sampler).
//!
//! The context is borrowed and read-only. A sampler owns its own
//! synchronization and must return its decision synchronously.

use std::time::SystemTime;

use crate::SinkId;
use crate::metadata::EventDescription;

/// Read-only inputs for one [`EventSampler`] decision.
#[derive(Debug)]
pub struct EventSamplingContext<'a> {
    description: &'a EventDescription,
    sink_id: SinkId,
    timestamp: SystemTime,
}

impl<'a> EventSamplingContext<'a> {
    /// Creates context for evaluating an [`EventSampler`] directly.
    #[must_use]
    pub const fn new(description: &'a EventDescription, sink_id: SinkId, timestamp: SystemTime) -> Self {
        Self {
            description,
            sink_id,
            timestamp,
        }
    }

    /// Returns the event's type-level description.
    #[must_use]
    pub const fn description(&self) -> &EventDescription {
        self.description
    }

    /// Returns the identity of the leaf sink making this decision.
    #[must_use]
    pub const fn sink_id(&self) -> SinkId {
        self.sink_id
    }

    /// Returns the timestamp this Sink assigned to the event.
    #[must_use]
    pub const fn timestamp(&self) -> SystemTime {
        self.timestamp
    }
}

/// What a leaf sink does with an event offered to its [`EventSampler`].
#[expect(
    clippy::exhaustive_enums,
    reason = "event sampling is a predicate with exactly two states, not an extensible workflow"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSamplingDecision {
    /// Run this Sink's interested processors.
    Continue,
    /// Run none of this Sink's processors for the event.
    Drop,
}

/// Decides whether an event reaches one leaf sink's processors.
///
/// Attach an implementation with
/// [`Sink::with_event_sampler`](crate::Sink::with_event_sampler). The same
/// sampler instance may be attached to multiple Sinks; use
/// [`EventSamplingContext::sink_id`] to distinguish them.
///
/// See the
/// [`event_sampling` example](https://github.com/microsoft/oxidizer/blob/main/crates/observed/examples/event_sampling.rs)
/// for a complete sampler and registration.
pub trait EventSampler: Send + Sync + 'static {
    /// Decides whether `event` continues to this Sink's processors.
    ///
    /// Called exactly once for every event the attached Sink is interested in.
    /// A Sink is interested when at least one of its processors reports
    /// interest. [`EventSamplingDecision::Continue`] runs normal processing;
    /// [`EventSamplingDecision::Drop`] runs no processor for that Sink.
    ///
    /// The call runs on the emitting thread. Emitting internal telemetry from
    /// here is not supported.
    fn sample(&self, event: &EventSamplingContext<'_>) -> EventSamplingDecision;
}
