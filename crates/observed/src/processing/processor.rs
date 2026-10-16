// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Event processor trait - the abstract dispatch contract.
//!
//! `observed` defines this trait but does not provide concrete log/metric
//! processors itself. Concrete processors that target `OTel` providers live
//! in separate destination crates; raw third-party processors implement
//! this trait directly.

use std::sync::Arc;

use super::EventView;
use crate::FlushError;
use crate::metadata::EventDescription;

/// A processor that receives lazy event views.
///
/// One processor typically represents one target (e.g. one log destination or
/// one metric destination). Each processor owns its own redactor
/// (typically a [`data_privacy::RedactionEngine`]) privately.
///
/// The emission infrastructure builds an [`EventView`] and passes it to
/// [`process()`](EventProcessor::process). The processor pulls only the
/// fields it needs - skipped fields never invoke their redaction closure.
///
/// Processors that only care about a subset of events (e.g. logs-only or
/// metrics-only) select them through
/// [`is_interested()`](EventProcessor::is_interested).
pub trait EventProcessor: Send + Sync {
    /// Decides whether this processor wants the event.
    ///
    /// Called at most once per emission for each Sink the processor belongs
    /// to, and not at all when that Sink's
    /// [`EventSampler`](crate::EventSampler) drops the event without asking.
    /// Keep it cheap.
    ///
    /// It is both the lazy-construction gate and the per-processor routing
    /// decision: the event closure runs only when at least one processor
    /// returns `true`, and a processor that returns `false` never receives the
    /// event even when a peer is interested.
    ///
    /// # Nested telemetry is not supported
    ///
    /// Do not emit from inside `is_interested()`, including to a different
    /// [`Sink`](crate::Sink). Report processor-internal failures through a
    /// non-`observed` channel instead.
    fn is_interested(&self, description: &EventDescription) -> bool;

    /// Processes an event by pulling fields and enrichments from the view.
    ///
    /// The processor owns its own redaction engine and passes it to getter
    /// closures when extracting field values.
    ///
    /// # Nested telemetry is not supported
    ///
    /// Do not emit from inside `process()`, including to a different
    /// [`Sink`](crate::Sink). Report processor-internal failures through a
    /// non-`observed` channel instead.
    fn process(&self, event: &EventView<'_>);

    /// Forces any buffered telemetry produced by this processor out to its
    /// final destination, surfacing errors. Idempotent and non-terminating -
    /// the processor remains usable after `flush()` returns. Implementors
    /// with nothing to flush should return `Ok(())`.
    ///
    /// [`Sink::flush`](crate::Sink::flush) iterates all registered
    /// processors and calls this; it reports every failure, not just the first.
    ///
    /// Build the error with `FlushError::new("my-processor", source)`: a sink
    /// only ever holds `dyn EventProcessor`, so the processor's identity has to
    /// be named here, at the implementation site.
    ///
    /// # Errors
    ///
    /// Returns a [`FlushError`] if flushing buffered telemetry to the final
    /// destination fails.
    fn flush(&self) -> Result<(), FlushError>;
}

impl<T: EventProcessor + ?Sized> EventProcessor for Arc<T> {
    fn is_interested(&self, description: &EventDescription) -> bool {
        (**self).is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        (**self).process(event);
    }

    fn flush(&self) -> Result<(), FlushError> {
        (**self).flush()
    }
}
