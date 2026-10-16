// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event sampling for [`Sink`](crate::Sink)s.
//!
//! A Sink with an attached [`EventSampler`] asks it whether the event goes
//! ahead before asking its processors. The sampler receives an
//! [`EventSamplingContext`] with the event description, and can ask through it
//! whether any of the Sink's processors wants the event. Attach one while
//! building a Sink with
//! [`SinkBuilder::event_sampler`](crate::SinkBuilder::event_sampler); a Sink
//! without one lets its processors decide.
//!
//! A sampler owns its own synchronization and must return its decision
//! synchronously.

mod context;
mod sampler;

pub use context::EventSamplingContext;
pub use sampler::{EventSampler, EventSamplingDecision};
