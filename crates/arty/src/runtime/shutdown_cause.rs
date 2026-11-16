// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A request or contained task panic that initiated runtime shutdown.
///
/// Returned by [`JoinError::shutdown_cause`](crate::task::JoinError::shutdown_cause).
/// The first shutdown cause remains authoritative, even if another failure
/// occurs during shutdown. Use [`std::error::Error::source`] on the join error
/// for details about an abnormal cause.
/// Other runtime failures have no classification in this enum.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ShutdownCause {
    /// Shutdown was requested without an error.
    Requested,
    /// A contained task panic initiated shutdown.
    ///
    /// The cancelled task did not itself panic. The task that panicked receives
    /// its own [`JoinError`](crate::task::JoinError) with the original payload.
    /// Cancelled or rejected tasks retain the panic message through their
    /// join error's [`std::error::Error::source`].
    Panic,
}
