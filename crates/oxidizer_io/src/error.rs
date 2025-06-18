// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thiserror::Error;
#[cfg(windows)]
use windows::Win32::Networking::WinSock::WSA_ERROR;

/// Any I/O error that may arise from either the low-level I/O operations provided by the
/// `oxidizer_io` crate or from higher-level I/O types that use these operations.
///
/// The type includes platform-specific enum variants. To write platform-neutral code, you
/// may ignore them and handle any unrecognized variants via variant-agnostic code.
///
/// # Thread safety
///
/// This type is thread-safe.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// An API contract was violated, e.g. an operation that is required to always consume all
    /// bytes only written some of the bytes.
    ///
    /// Note: an API contract violation that indicates memory safety has been violated will
    /// panic instead of returning an error result.
    #[error("contract violation: {0}")]
    ContractViolation(String),

    /// The operation was canceled due to a signal indicating that it is no longer relevant.
    ///
    /// This may be used, for example, when an I/O primitive is already closed by the
    /// time an operation starts or when an out of band cancel request is received.
    #[error("operation canceled")]
    Canceled,

    /// (Windows only) [The Windows Sockets API returned an error result][1] that is represented
    /// transparently.
    ///
    /// This is used if there is no platform-agnostic enum variant to report for a
    /// situation either in this type or in `std::io::ErrorKind`.
    ///
    /// [1]: https://learn.microsoft.com/en-us/windows/win32/winsock/windows-sockets-error-codes-2
    #[cfg(windows)]
    #[error("Winsock error {}", .0.0)]
    Winsock(WSA_ERROR),

    /// (Windows only) A Windows API returned an error result that is represented transparently.
    ///
    /// This is used if there is no platform-agnostic enum variant to report for a
    /// situation either in this type or in `std::io::ErrorKind`.
    #[cfg(windows)]
    #[error(transparent)]
    Windows(#[from] windows::core::Error),

    /// We are forwarding an error received from the standard library's I/O APIs.
    #[error(transparent)]
    StdIo(#[from] std::io::Error),

    /// We are forwarding an error of unknown type from an unspecified source.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// A specialized `Result` for use with I/O operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Represents the Oxidizer I/O subsystem error as a standard I/O error.
/// This is often used when interoperating with other libraries that expect standard I/O errors.
impl From<Error> for std::io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::StdIo(error) => error,
            _ => Self::other(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use static_assertions::assert_impl_all;

    use super::*;

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(Error: Send, Sync);
    }

    #[test]
    fn inspect_stdio_error() {
        let e = Error::StdIo(std::io::Error::new(
            ErrorKind::AlreadyExists,
            "hey what did you do",
        ));

        match e {
            Error::StdIo(e) => {
                assert_eq!(e.kind(), ErrorKind::AlreadyExists);
                assert_eq!(e.to_string(), "hey what did you do");
            }
            _ => panic!("unexpected error variant"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn inspect_windows_error() {
        use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

        let e = Error::Windows(windows::core::Error::from_hresult(
            ERROR_INSUFFICIENT_BUFFER.to_hresult(),
        ));

        match e {
            Error::Windows(e) => {
                assert_eq!(e.code(), ERROR_INSUFFICIENT_BUFFER.to_hresult());
            }
            _ => panic!("unexpected error variant"),
        }
    }

    #[cfg(not(miri))] // Miri cannot handle Windows error message formatting.
    #[cfg(windows)]
    #[test]
    fn inspect_windows_error_platform_neutral() {
        use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

        let e = Error::Windows(windows::core::Error::from_hresult(
            ERROR_INSUFFICIENT_BUFFER.to_hresult(),
        ));

        match e {
            Error::StdIo(e) => {
                assert_eq!(e.kind(), ErrorKind::AlreadyExists);
                assert_eq!(e.to_string(), "hey what did you do");
            }
            // We do not need to use platform-specific types to inspect a platform-specific error.
            other => assert_eq!(
                other.to_string(),
                "The data area passed to a system call is too small. (0x8007007A)"
            ),
        }
    }

    #[test]
    fn into_stdio_error() {
        let e = Error::ContractViolation("hey what did you do".to_string());

        let io_error: std::io::Error = e.into();
        assert_eq!(io_error.kind(), ErrorKind::Other);

        let e = Error::StdIo(std::io::Error::new(
            ErrorKind::AlreadyExists,
            "hey what did you do",
        ));

        let io_error: std::io::Error = e.into();
        assert_eq!(io_error.kind(), ErrorKind::AlreadyExists);
    }
}