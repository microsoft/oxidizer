// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// What to do when the executor shutdown timeout is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownTimeoutBehavior {
    /// Terminate the process after reporting the problem.
    ///
    /// This is the only option available to consumers outside this crate because
    /// this is the only option that does not lead to a memory safety violation.
    TerminateProcess,

    /// Panic after reporting the problem.
    ///
    /// This is only used in unit tests but is not available externally to discourage
    /// anyone attempting to recover from a shutdown timeout (which is not possible
    /// in a way that is guaranteed to avoid a memory safety violation).
    #[cfg(test)]
    Panic,
}
