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
    /// All other error kinds will return [`RecoveryInfo::never`], as they represent permanent
    /// conditions that retrying cannot resolve (e.g. file not found, permission denied, invalid
    /// data).
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
    use std::io::ErrorKind;

    use crate::RecoveryInfo;

    #[test]
    fn from_io_error_kind_never() {
        assert_eq!(RecoveryInfo::from(ErrorKind::NotFound), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::PermissionDenied), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::AlreadyExists), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::InvalidData), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::InvalidInput), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::UnexpectedEof), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::WriteZero), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::Unsupported), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::OutOfMemory), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::NotADirectory), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::IsADirectory), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::DirectoryNotEmpty), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::ReadOnlyFilesystem), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::StorageFull), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::NotSeekable), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::QuotaExceeded), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::FileTooLarge), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::ExecutableFileBusy), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::Deadlock), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::CrossesDevices), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::TooManyLinks), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::InvalidFilename), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::ArgumentListTooLong), RecoveryInfo::never());
        assert_eq!(RecoveryInfo::from(ErrorKind::Other), RecoveryInfo::never());
    }

    #[test]
    fn from_io_error_kind_retry() {
        assert_eq!(RecoveryInfo::from(ErrorKind::WouldBlock), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::TimedOut), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::ConnectionReset), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::ConnectionAborted), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::NotConnected), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::ConnectionRefused), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::AddrInUse), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::AddrNotAvailable), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::BrokenPipe), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::Interrupted), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::StaleNetworkFileHandle), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::ResourceBusy), RecoveryInfo::retry());
    }

    #[test]
    fn from_io_error_kind_unavailable() {
        assert_eq!(RecoveryInfo::from(ErrorKind::HostUnreachable), RecoveryInfo::unavailable());
        assert_eq!(RecoveryInfo::from(ErrorKind::NetworkUnreachable), RecoveryInfo::unavailable());
        assert_eq!(RecoveryInfo::from(ErrorKind::NetworkDown), RecoveryInfo::unavailable());
    }
}
