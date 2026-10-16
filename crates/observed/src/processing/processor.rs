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
    /// Returns whether this processor is interested in the described event.
    ///
    /// Uses the event description rather than inspecting event fields.
    ///
    /// Called while routing events and, for lazy typed events, **before**
    /// construction. It may run more than once per emission, including through
    /// composite sinks. Keep it cheap, and let the answer depend only on
    /// `description` and on state that changes at most once, such as a
    /// `OnceLock` filled during initialization. A sampler, rate limiter, or any
    /// filter whose answer varies per call belongs in
    /// [`process()`](Self::process), which runs exactly once per delivery.
    ///
    /// Also called by [`Sink::is_interested`](crate::Sink::is_interested),
    /// which aggregates processor interest independently of emission.
    ///
    /// Interest is advisory for the current check, not the processor's lifetime.
    /// Initialization may change selection for subsequent emissions. Checks do
    /// not form an atomic snapshot across processors, and earlier decisions
    /// need not be revisited during the same emission.
    ///
    /// Interest gates lazy construction and selects recipients. If every
    /// processor declines admission, the event closure is never invoked.
    /// Routing can check interest independently of admission.
    fn is_interested(&self, description: &EventDescription) -> bool;

    /// Processes an event by pulling fields and enrichments from the view.
    ///
    /// The processor owns its own redaction engine and passes it to getter
    /// closures when extracting field values.
    ///
    /// # Nested telemetry is not supported
    ///
    /// Emitting from inside `process()` is silently dropped, including to a
    /// different [`Sink`](crate::Sink): a thread-wide reentrancy guard skips
    /// any nested `emit!` for the duration of the outer emission. Report
    /// processor-internal failures through a non-`observed` channel instead.
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
