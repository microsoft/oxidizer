// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The drop guard that emits the event of one request, on both sides.

use observed::Sink;
use observed::context::Transfer;
use tick::{SimpleClock, Stopwatch};

/// One request of a side, client or server: the values that its events record, and how it emits them.
pub(crate) trait EmitEvents {
    /// How a request of this side ends.
    type RequestEnd<'a>;

    /// The end of a request that the caller dropped before it ended.
    const ABANDONED: Self::RequestEnd<'static>;

    /// Emits the events of the request that ended with `end` after `duration_secs` seconds.
    fn emit_events(&self, end: Self::RequestEnd<'_>, duration_secs: f64);
}

/// Drop guard that emits the event of one request, also when the caller drops the request future.
///
/// The middleware creates the guard in `execute`, so the duration starts at the call. [`emit`] emits the event
/// of the request, once. If the guard is dropped before that, it emits the abandoned event of its side.
///
/// [`emit`]: Self::emit
pub(crate) struct TelemetryDropGuard<R: EmitEvents> {
    pub(crate) request: R,
    stopwatch: Stopwatch,
    context_transfer: Transfer,
    finished: bool,
}

impl<R: EmitEvents> TelemetryDropGuard<R> {
    /// Starts the duration of `request`, and keeps the enrichment of the caller for its event.
    pub(crate) fn new(request: R, sink: &Sink, clock: &SimpleClock) -> Self {
        Self {
            request,
            stopwatch: clock.stopwatch(),
            context_transfer: sink.transfer_context(),
            finished: false,
        }
    }

    pub(crate) fn emit(&mut self, end: R::RequestEnd<'_>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let duration_secs = self.stopwatch.elapsed().as_secs_f64();
        let _enrichment = self.context_transfer.apply_current_thread();
        self.request.emit_events(end, duration_secs);
    }
}

impl<R: EmitEvents> Drop for TelemetryDropGuard<R> {
    fn drop(&mut self) {
        // Emit nothing while the thread panics, because a panic in a processor now aborts the process.
        if !std::thread::panicking() {
            self.emit(R::ABANDONED);
        }
    }
}
