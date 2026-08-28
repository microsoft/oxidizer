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
    /// Called **before** the event is constructed, and again while routing it,
    /// so it may run more than once per emission - and once per child for a
    /// composite sink. Keep it cheap, and let the answer depend only on
    /// `description` and on state that changes at most once, such as a
    /// `OnceLock` filled during initialization. A sampler, rate limiter, or any
    /// filter whose answer varies per call belongs in
    /// [`process()`](Self::process), which runs exactly once per delivery.
    ///
    /// It is both the lazy-construction gate and the per-processor routing
    /// decision: if **all** processors return `false` the event closure is
    /// never invoked, and a processor that returns `false` never receives the
    /// event even when a peer is interested.
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
    /// with nothing to flush use the default no-op implementation.
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
    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
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
