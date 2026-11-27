// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Poll;

use arty::runtime::hook::{HookFn, TaskObserver, TaskSpawn};
use arty::runtime::{RuntimeBuilder, RuntimeControl};
use observed::Sink;
use observed::context::{Transfer, TransferGuard};

/// Configures Observed sink integration for an Arty runtime.
///
/// Pass an `ObservedOptions`, an owned [`Sink`], or a shared `&Sink` to
/// [`ObservedExt::enable_observed`]. The shared form clones the sink handle.
///
/// Its private fields let later releases add configuration without changing
/// [`ObservedExt::enable_observed`].
///
/// Clone this value when another owner needs it. It intentionally does not
/// implement `Copy`, so later configuration fields do not inherit implicit
/// copying without an explicit API decision.
#[derive(Clone, Debug)]
pub struct ObservedOptions {
    /// Supplies enrichment for covered tasks.
    sink: Sink,
}

impl ObservedOptions {
    /// Selects the sink that supplies enrichment for covered async tasks.
    #[inline]
    #[must_use]
    pub const fn new(sink: Sink) -> Self {
        Self { sink }
    }

    /// Returns the sink that supplies the enrichment context.
    #[inline]
    #[must_use]
    pub const fn sink(&self) -> &Sink {
        &self.sink
    }
}

impl From<Sink> for ObservedOptions {
    #[inline]
    fn from(sink: Sink) -> Self {
        Self::new(sink)
    }
}

impl From<&Sink> for ObservedOptions {
    fn from(sink: &Sink) -> Self {
        Self::new(sink.clone())
    }
}

/// Adds Observed sink integration to an Arty runtime.
///
/// The implementation for [`RuntimeBuilder`] registers the task-spawn hook
/// before startup. The implementation for a [`RuntimeControl`] reference
/// registers it on a live runtime. Builder registration covers every async
/// task. Live registration runs immediately on the calling worker and is
/// queued on other workers, so async tasks submitted there before the queue is
/// processed are not covered. Use builder registration when every async task
/// must be covered.
///
/// The current integration transfers sink enrichment into each covered async
/// task. Blocking functions submitted through `spawn_blocking` are not covered.
pub trait ObservedExt: private::Sealed {
    /// The result of enabling Observed sink integration.
    type Output;

    /// Enables the configured sink integration.
    ///
    /// On [`RuntimeBuilder`], this covers every async task. On live
    /// [`RuntimeControl`], registration is queued on other workers and does not
    /// cover async tasks that they submit before processing that queue.
    /// Blocking functions submitted through `spawn_blocking` are not covered.
    fn enable_observed(self, options: impl Into<ObservedOptions>) -> Self::Output;
}

impl ObservedExt for RuntimeBuilder {
    type Output = Self;

    fn enable_observed(self, options: impl Into<ObservedOptions>) -> Self::Output {
        self.on(task_spawn(options.into().sink))
    }
}

impl ObservedExt for &RuntimeControl {
    type Output = ();

    fn enable_observed(self, options: impl Into<ObservedOptions>) {
        self.on(task_spawn(options.into().sink));
    }
}

/// Captures each submission from the sink without exposing hook payloads.
fn task_spawn(sink: Sink) -> HookFn<TaskSpawn, Sink> {
    HookFn::new(sink, capture_enrichment)
}

/// Captures the submitting thread's context before Arty moves the transfer.
fn capture_enrichment(event: &mut TaskSpawn, sink: &Sink) {
    event.observe_with(sink.transfer_context(), ObservedTask::new);
}

/// Applies one task's captured enrichment while Arty polls that task.
///
/// Arty constructs this observer on the destination worker. The observer
/// remains with the task across polls, so only its captured transfer moves.
struct ObservedTask {
    /// Captured enrichment that remains available for every task poll.
    transfer: Transfer,
    /// Preserves the worker's prior context during the current poll only.
    guard: Option<TransferGuard>,
}

impl ObservedTask {
    fn new(transfer: Transfer) -> Self {
        Self { transfer, guard: None }
    }
}

impl TaskObserver for ObservedTask {
    fn poll_start(&mut self) {
        self.guard = Some(self.transfer.apply_current_thread());
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {
        self.guard = None;
    }
}

mod private {
    use arty::runtime::{RuntimeBuilder, RuntimeControl};

    pub trait Sealed {}

    impl Sealed for RuntimeBuilder {}
    impl Sealed for &RuntimeControl {}
}

#[cfg(all(test, not(miri)))]
mod tests {
    use std::alloc::System;
    use std::hint::black_box;

    use alloc_tracker::{Allocator, Session};

    use super::*;

    /// Routes allocations from the poll-scope test to the measurement session.
    #[global_allocator]
    static ALLOCATOR: Allocator<System> = Allocator::system();

    /// Arbitrarily repeats a short poll scope to make allocation accounting stable.
    const MEASURED_POLLS: usize = 1_000;

    /// Verifies the public contract that a repeated single-sink poll does not allocate.
    #[test]
    fn repeated_single_sink_poll_scopes_do_not_allocate() {
        let sink = Sink::builder("allocation_test").build();
        let mut observer = ObservedTask::new(sink.transfer_context());
        observer.poll_start();
        observer.poll_end(Poll::Pending);

        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("poll_scope");
        {
            let _span = operation.measure_thread().iterations(MEASURED_POLLS as u64);
            for _ in 0..MEASURED_POLLS {
                observer.poll_start();
                black_box(());
                observer.poll_end(Poll::Pending);
            }
        }

        let allocations = session
            .to_report()
            .operations()
            .find_map(|(name, operation)| (name == "poll_scope").then(|| operation.total_allocations_count()))
            .unwrap();
        assert_eq!(allocations, 0);
    }
}
