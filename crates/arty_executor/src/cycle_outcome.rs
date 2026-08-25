// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The outcome of executing one processing cycle of the async task executor.
///
/// This may have an impact on how the executor's owner should proceed with further processing
/// cycles or whether that is even possible.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum CycleOutcome {
    /// The processing cycle was completed and the async task executor is requesting a new
    /// processing cycle as soon as possible because it believes it already has more work to do.
    Continue,

    /// The processing cycle was completed and the async task executor is confident that no
    /// further progress can be made at this time.
    ///
    /// The owner of the executor should avoid executing further processing cycles until there
    /// is reason to suspect additional progress can be made. Examples include:
    /// * The owner has enqueued new tasks.
    /// * An I/O operation has completed.
    /// * Something sends a wake signal via the `owner_waker` provided to
    ///   [`ExecutorBuilder::owner_waker()`][1].
    ///
    /// Even in the absence of the above factors, it is desirable to execute new processing cycles
    /// periodically, merely because there may (in theory) be events that create more work for the
    /// executor but which fail to be detected via any of the above signals. Therefore, a time-based
    /// periodic execution cycle may serve as a useful fail-safe.
    ///
    /// [1]: crate::ExecutorBuilder::owner_waker
    Suspend,

    /// The executor has completed shutdown and is ready to be dropped.
    ///
    /// Additional processing cycles are unnecessary but harmless and continue returning
    /// `Shutdown`.
    Shutdown,
}
