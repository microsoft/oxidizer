// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! IO recovery information for [`RecoveryInfo`].

use std::io::ErrorKind;

use crate::RecoveryInfo;

impl From<ErrorKind> for RecoveryInfo {
    /// Creates recovery information from an IO error kind.
    ///
    /// This is an opinionated default classification based on common IO error kinds. The exact
    /// classification of each variant may change in future versions as understanding of failure
    /// modes evolves. If the default classification does not meet your requirements, implement
    /// your own conversion logic instead of relying on this one.
    ///
    /// # Retry
    ///
    /// The following are considered transient and will return [`RecoveryInfo::retry`]. These
    /// errors typically resolve quickly (seconds to minutes) without external intervention:
    ///
    /// - [`ErrorKind::WouldBlock`]: resource temporarily unavailable
    /// - [`ErrorKind::TimedOut`]: individual request timeout
    /// - [`ErrorKind::ConnectionReset`]: connection dropped by remote
    /// - [`ErrorKind::ConnectionAborted`]: connection terminated by remote
    /// - [`ErrorKind::NotConnected`]: socket not yet connected
    /// - [`ErrorKind::ConnectionRefused`]: server not listening (e.g. service restarting)
    /// - [`ErrorKind::AddrInUse`]: address temporarily occupied
    /// - [`ErrorKind::AddrNotAvailable`]: interface temporarily unavailable
    /// - [`ErrorKind::BrokenPipe`]: reader closed, reconnect may help
    /// - [`ErrorKind::Interrupted`]: signal interrupted syscall
    /// - [`ErrorKind::StaleNetworkFileHandle`]: NFS handle invalidated, re-open may succeed
    /// - [`ErrorKind::ResourceBusy`]: resource locked, may become available shortly
    ///
    /// # Unavailable
    ///
    /// The following indicate infrastructure-level problems and will return
    /// [`RecoveryInfo::unavailable`]. These errors may take an extended period to resolve
    /// (minutes to hours) and suggest circuit-breaker patterns or fallback strategies:
    ///
    /// - [`ErrorKind::HostUnreachable`]: routing or infrastructure problem
    /// - [`ErrorKind::NetworkUnreachable`]: entire network segment unreachable
    /// - [`ErrorKind::NetworkDown`]: network interface is down
    ///
    /// # Never
    ///
    /// All other error kinds will return [`RecoveryInfo::never`] by default. Many of these
    /// represent permanent conditions that retrying cannot resolve (e.g. file not found, permission
    /// denied, invalid data), but the catch-all also covers ambiguous variants like
    /// [`ErrorKind::Other`] that may include transient errors. If your use case encounters
    /// transient errors reported as uncategorized kinds, implement your own conversion logic.
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::WouldBlock
            | ErrorKind::TimedOut
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::NotConnected
            | ErrorKind::ConnectionRefused
            | ErrorKind::AddrInUse
            | ErrorKind::AddrNotAvailable
            | ErrorKind::BrokenPipe
            | ErrorKind::Interrupted
            | ErrorKind::StaleNetworkFileHandle
            | ErrorKind::ResourceBusy => Self::retry(),

            ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable | ErrorKind::NetworkDown => Self::unavailable(),

            _ => Self::never(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// All [`ErrorKind`] variants paired with their [`RecoveryInfo`] classification.
    ///
    /// Used by tests to snapshot the full mapping. Keep this in sync when new variants are added.
    const ALL_ERROR_KINDS: &[ErrorKind] = &[
        // retry
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::ConnectionReset,
        ErrorKind::ConnectionAborted,
        ErrorKind::NotConnected,
        ErrorKind::ConnectionRefused,
        ErrorKind::AddrInUse,
        ErrorKind::AddrNotAvailable,
        ErrorKind::BrokenPipe,
        ErrorKind::Interrupted,
        ErrorKind::StaleNetworkFileHandle,
        ErrorKind::ResourceBusy,
        // unavailable
        ErrorKind::HostUnreachable,
        ErrorKind::NetworkUnreachable,
        ErrorKind::NetworkDown,
        // never
        ErrorKind::NotFound,
        ErrorKind::PermissionDenied,
        ErrorKind::AlreadyExists,
        ErrorKind::InvalidData,
        ErrorKind::InvalidInput,
        ErrorKind::UnexpectedEof,
        ErrorKind::WriteZero,
        ErrorKind::Unsupported,
        ErrorKind::OutOfMemory,
        ErrorKind::NotADirectory,
        ErrorKind::IsADirectory,
        ErrorKind::DirectoryNotEmpty,
        ErrorKind::ReadOnlyFilesystem,
        ErrorKind::StorageFull,
        ErrorKind::NotSeekable,
        ErrorKind::QuotaExceeded,
        ErrorKind::FileTooLarge,
        ErrorKind::ExecutableFileBusy,
        ErrorKind::Deadlock,
        ErrorKind::CrossesDevices,
        ErrorKind::TooManyLinks,
        ErrorKind::InvalidFilename,
        ErrorKind::ArgumentListTooLong,
        ErrorKind::Other,
    ];

    #[test]
    fn from_io_error_kind() {
        let classifications: Vec<_> = ALL_ERROR_KINDS.iter().map(|&kind| (kind, RecoveryInfo::from(kind))).collect();

        insta::assert_debug_snapshot!(classifications);
    }
}
