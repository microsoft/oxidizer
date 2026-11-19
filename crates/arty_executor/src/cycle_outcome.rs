// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The result of one executor processing cycle.
///
/// The owner uses this value to decide when to run another cycle.
#[derive(Debug)]
pub enum CycleOutcome {
    /// The executor has more work that it can do now.
    Continue,

    /// The executor cannot make more progress now.
    ///
    /// Wait for a reason to run another cycle. A reason can be:
    ///
    /// * The owner has enqueued new tasks.
    /// * An I/O operation has completed.
    /// * A component sends a wake signal through the `owner_waker` from
    ///   [`ExecutorBuilder::owner_waker()`][1].
    ///
    /// The owner can also run periodic cycles as a fallback for missed wake
    /// signals.
    ///
    /// [1]: crate::ExecutorBuilder::owner_waker
    Suspend,

    /// The executor has completed graceful shutdown and owns no tasks.
    ///
    /// Later cycles do no work and return `Shutdown` with the first shutdown
    /// cause.
    Shutdown(crate::ShutdownCause),
}
