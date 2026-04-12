// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! IO recovery information for [`RecoveryInfo`].

use std::io::ErrorKind;

use crate::RecoveryInfo;

impl From<ErrorKind> for RecoveryInfo {
    /// Creates recovery information from an IO error kind.
    ///
    /// This is opinionated detection based on common transient IO error kinds. The following
    /// are considered recoverable and will return [`RecoveryInfo::retry`]:
    ///
    /// - [`ErrorKind::WouldBlock`]
    /// - [`ErrorKind::TimedOut`]
    /// - [`ErrorKind::ConnectionReset`]
    /// - [`ErrorKind::ConnectionAborted`]
    /// - [`ErrorKind::NotConnected`]
    /// - [`ErrorKind::ConnectionRefused`]
    /// - [`ErrorKind::AddrInUse`]
    /// - [`ErrorKind::AddrNotAvailable`]
    /// - [`ErrorKind::BrokenPipe`]
    /// - [`ErrorKind::Interrupted`]
    /// - [`ErrorKind::HostUnreachable`]
    /// - [`ErrorKind::NetworkUnreachable`]
    /// - [`ErrorKind::NetworkDown`]
    /// - [`ErrorKind::StaleNetworkFileHandle`]
    /// - [`ErrorKind::ResourceBusy`]
    ///
    /// All other error kinds will return [`RecoveryInfo::never`].
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
            | ErrorKind::HostUnreachable
            | ErrorKind::NetworkUnreachable
            | ErrorKind::NetworkDown
            | ErrorKind::StaleNetworkFileHandle
            | ErrorKind::ResourceBusy => Self::retry(),
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
        // Test some non-retryable ErrorKinds
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
        // Test all retryable ErrorKinds
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
        assert_eq!(RecoveryInfo::from(ErrorKind::HostUnreachable), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::NetworkUnreachable), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::NetworkDown), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::StaleNetworkFileHandle), RecoveryInfo::retry());
        assert_eq!(RecoveryInfo::from(ErrorKind::ResourceBusy), RecoveryInfo::retry());
    }
}
