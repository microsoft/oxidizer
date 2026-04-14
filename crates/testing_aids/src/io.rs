// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::ErrorKind;

/// All [`ErrorKind`] variants useful for exhaustive testing.
///
/// Keep this in sync when new stable variants are added to [`ErrorKind`].
pub const ALL_ERROR_KINDS: &[ErrorKind] = &[
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
    ErrorKind::HostUnreachable,
    ErrorKind::NetworkUnreachable,
    ErrorKind::NetworkDown,
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
