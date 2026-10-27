// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recovery information for I/O errors.

use std::io::ErrorKind;

use crate::RecoveryInfo;

impl From<ErrorKind> for RecoveryInfo {
    /// Creates recovery information from an I/O error kind.
    ///
    /// This conversion defines default classifications for common I/O errors.
    /// A future version can change a classification. If a default does not meet
    /// your requirements, implement a conversion for your error type.
    ///
    /// # Retry
    ///
    /// These transient errors return [`RecoveryInfo::retry`]. They usually
    /// resolve within seconds or minutes without external action:
    ///
    /// - [`ErrorKind::WouldBlock`]: A resource is temporarily unavailable.
    /// - [`ErrorKind::TimedOut`]: One request timed out.
    /// - [`ErrorKind::ConnectionReset`]: The remote endpoint dropped the connection.
    /// - [`ErrorKind::ConnectionAborted`]: The remote endpoint ended the connection.
    /// - [`ErrorKind::NotConnected`]: The socket is not connected.
    /// - [`ErrorKind::ConnectionRefused`]: The server is not listening.
    /// - [`ErrorKind::AddrInUse`]: The address is temporarily in use.
    /// - [`ErrorKind::AddrNotAvailable`]: The network interface is temporarily unavailable.
    /// - [`ErrorKind::BrokenPipe`]: The reader closed the pipe. A new connection can help.
    /// - [`ErrorKind::Interrupted`]: A signal interrupted the system call.
    /// - [`ErrorKind::StaleNetworkFileHandle`]: An NFS handle is invalid. Opening it again can succeed.
    /// - [`ErrorKind::ResourceBusy`]: A resource is locked. It can become available soon.
    /// - [`ErrorKind::Deadlock`]: The OS prevented a deadlock. A different lock order can succeed.
    /// - [`ErrorKind::ExecutableFileBusy`]: A process is running the file.
    /// - [`ErrorKind::QuotaExceeded`]: Usage reached a resource quota.
    ///
    /// # Unavailable
    ///
    /// These infrastructure errors return [`RecoveryInfo::unavailable`]. They
    /// can take minutes or hours to resolve. Callers can use a circuit breaker
    /// or a fallback:
    ///
    /// - [`ErrorKind::HostUnreachable`]: A routing or infrastructure problem blocks the host.
    /// - [`ErrorKind::NetworkUnreachable`]: The system cannot reach the network segment.
    /// - [`ErrorKind::NetworkDown`]: The network interface is down.
    /// - [`ErrorKind::StorageFull`]: The disk or storage is full.
    /// - [`ErrorKind::OutOfMemory`]: The system has no available memory.
    ///
    /// # Never
    ///
    /// All other error kinds return [`RecoveryInfo::never`] by default.
    /// Many of these errors are permanent, such as missing files, denied
    /// permissions, or invalid data. This group also includes ambiguous variants
    /// such as [`ErrorKind::Other`], which can contain transient errors.
    /// Implement a conversion for your error type when it uses these variants.
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
            | ErrorKind::ResourceBusy
            | ErrorKind::Deadlock
            | ErrorKind::ExecutableFileBusy
            | ErrorKind::QuotaExceeded => Self::retry(),

            ErrorKind::HostUnreachable
            | ErrorKind::NetworkUnreachable
            | ErrorKind::NetworkDown
            | ErrorKind::StorageFull
            | ErrorKind::OutOfMemory => Self::unavailable(),

            _ => Self::never(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use testing_aids::ALL_ERROR_KINDS;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn from_io_error_kind() {
        let classifications: Vec<_> = ALL_ERROR_KINDS.iter().map(|&kind| (kind, RecoveryInfo::from(kind))).collect();

        insta::assert_debug_snapshot!(classifications);
    }
}
