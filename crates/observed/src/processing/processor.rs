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
use crate::metadata::EventDescription;

/// A processor that receives lazy event views.
///
/// One processor typically represents one target (e.g. one log destination or
/// one metric destination). Each processor owns its own redaction engine
/// (typically a [`data_privacy::RedactionEngine`]) privately.
///
/// The emission infrastructure builds an [`EventView`] and passes it to
/// [`process()`](EventProcessor::process). The processor pulls only the
/// fields it needs - skipped fields never invoke their redaction closure.
///
/// Processors that only care about a subset of events (e.g. logs-only or
/// metrics-only) should also filter inside `process()` using
/// [`EventView::description()`].
pub trait EventProcessor: Send + Sync {
    /// Fast prefilter using compile-time event metadata.
    ///
    /// Called **before** the event is constructed. If **all** processors
    /// return `false`, the event closure is never invoked (lazy construction
    /// optimization). This is not used for per-processor routing - every
    /// processor that has at least one peer interested will receive the
    /// event and should filter inside [`process()`](Self::process).
    fn is_interested(&self, description: &EventDescription) -> bool;

    /// Processes an event by pulling fields and enrichments from the view.
    ///
    /// The processor owns its own redaction engine and passes it to getter
    /// closures when extracting field values.
    fn process(&self, event: &EventView<'_>);

    /// Forces any buffered telemetry produced by this processor out to its
    /// final destination, surfacing errors. Idempotent and non-terminating -
    /// the processor remains usable after `flush()` returns. Implementors
    /// with nothing to flush should return `Ok(())`.
    ///
    /// [`Sink::flush`](crate::Sink::flush) iterates all registered
    /// processors and calls this; it returns the first error encountered.
    ///
    /// # Errors
    ///
    /// Returns an error if flushing buffered telemetry to the final
    /// destination fails.
    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

impl<T: EventProcessor + ?Sized> EventProcessor for Arc<T> {
    fn is_interested(&self, description: &EventDescription) -> bool {
        (**self).is_interested(description)
    }

    fn process(&self, event: &EventView<'_>) {
        (**self).process(event);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        (**self).flush()
    }
}
