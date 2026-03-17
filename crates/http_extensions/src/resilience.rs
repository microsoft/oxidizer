// Copyright (c) Microsoft Corporation.

//! Resilience helpers for HTTP workloads.

use std::io::ErrorKind;

use recoverable::RecoveryInfo;

/// Detects if an IO error kind is recoverable.
///
/// This is opinionated detection based on common transient IO error kinds.
// TODO: should this be moved to recoverable crate?
#[must_use]
pub fn detect_io_recovery(kind: ErrorKind) -> RecoveryInfo {
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
        | ErrorKind::Interrupted => RecoveryInfo::retry(),
        _ => RecoveryInfo::never(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_io_recovery_never() {
        // Test some non-retryable ErrorKinds
        assert_eq!(detect_io_recovery(ErrorKind::NotFound), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::PermissionDenied), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::AlreadyExists), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::InvalidData), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::InvalidInput), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::UnexpectedEof), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::WriteZero), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::Unsupported), RecoveryInfo::never());
        assert_eq!(detect_io_recovery(ErrorKind::OutOfMemory), RecoveryInfo::never());
    }

    #[test]
    fn detect_io_recovery_retry() {
        // Test all retryable ErrorKinds
        assert_eq!(detect_io_recovery(ErrorKind::WouldBlock), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::TimedOut), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::ConnectionReset), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::ConnectionAborted), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::NotConnected), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::ConnectionRefused), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::AddrInUse), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::AddrNotAvailable), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::BrokenPipe), RecoveryInfo::retry());
        assert_eq!(detect_io_recovery(ErrorKind::Interrupted), RecoveryInfo::retry());
    }
}
