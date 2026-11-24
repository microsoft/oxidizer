// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Poll;

use arty::runtime::hook::{HookFn, TaskObserver, TaskSpawn};
use arty::runtime::{RuntimeBuilder, RuntimeControl};
use observed::Sink;
use observed::context::{Transfer, TransferGuard};
use thread_aware::ThreadAware;

/// Configures Observed sink integration for an Arty runtime.
///
/// This type is non-exhaustive so later releases can add configuration without
/// changing [`ObservedExt::enable_observed`].
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct ObservedOptions {
    sink: Sink,
}

impl ObservedOptions {
    /// Creates configuration for `sink`.
    #[must_use]
    pub const fn new(sink: Sink) -> Self {
        Self { sink }
    }

    /// Returns the sink that supplies the enrichment context.
    #[must_use]
    pub const fn sink(&self) -> &Sink {
        &self.sink
    }
}

impl From<Sink> for ObservedOptions {
    fn from(sink: Sink) -> Self {
        Self::new(sink)
    }
}

/// Adds Observed sink integration to an Arty runtime.
///
/// The implementation for [`RuntimeBuilder`] registers the task-spawn hook
/// before startup. The implementation for a [`RuntimeControl`] reference
/// registers it on a live runtime. Builder registration covers every task.
/// Live registration runs immediately on the calling worker and is queued on
/// other workers, so tasks submitted there before the queue is processed are
/// not covered. Use builder registration when every task must be covered.
///
/// The current integration transfers sink enrichment into each covered task.
pub trait ObservedExt: private::Sealed {
    /// The result of enabling Observed sink integration.
    type Output;

    /// Enables the configured sink integration.
    ///
    /// On [`RuntimeBuilder`], this covers every task. On live
    /// [`RuntimeControl`], registration is queued on other workers and does not
    /// cover tasks that they submit before processing that queue.
    fn enable_observed(self, options: impl Into<ObservedOptions>) -> Self::Output;
}

impl ObservedExt for RuntimeBuilder {
    type Output = Self;

    fn enable_observed(self, options: impl Into<ObservedOptions>) -> Self::Output {
        self.on(task_spawn(options.into()))
    }
}

impl ObservedExt for &RuntimeControl {
    type Output = ();

    fn enable_observed(self, options: impl Into<ObservedOptions>) {
        self.on(task_spawn(options.into()));
    }
}

fn task_spawn(options: ObservedOptions) -> HookFn<TaskSpawn, ObservedOptions> {
    HookFn::new(options, capture_enrichment)
}

fn capture_enrichment(event: &mut TaskSpawn, options: &ObservedOptions) {
    event.observe_with(options.sink.transfer_context(), ObservedTask::new);
}

struct ObservedTask {
    guard: TransferGuard,
}

impl ObservedTask {
    fn new(transfer: Transfer) -> Self {
        Self {
            guard: transfer.into_inactive_guard(),
        }
    }
}

impl TaskObserver for ObservedTask {
    fn poll_start(&mut self) {
        self.guard.apply_current_thread();
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {
        self.guard.restore_current_thread();
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

    #[global_allocator]
    static ALLOCATOR: Allocator<System> = Allocator::system();

    const MEASURED_POLLS: usize = 1_000;

    #[test]
    fn repeated_composite_poll_scopes_do_not_allocate() {
        let sink = Sink::composite([
            Sink::builder("allocation_test_1").build(),
            Sink::builder("allocation_test_2").build(),
            Sink::builder("allocation_test_3").build(),
            Sink::builder("allocation_test_4").build(),
        ]);
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
