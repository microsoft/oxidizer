// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use http_extensions::HttpError;
use ohno::ErrorLabel;
use recoverable::RecoveryInfo;

use crate::error_labels;

pub(crate) type Result<T> = std::result::Result<T, WinHttpError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WinHttpOperation {
    CloseHandle,
    Connect,
    Open,
    OpenRequest,
    QueryDataAvailable,
    QueryHeaders,
    QueryOption,
    ReadData,
    ReceiveResponse,
    SendRequest,
    SetOption,
    SetStatusCallback,
    SetTimeouts,
    WriteData,
}

impl fmt::Display for WinHttpOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CloseHandle => "closing a WinHTTP handle",
            Self::Connect => "creating a WinHTTP connection handle",
            Self::Open => "opening a WinHTTP session",
            Self::OpenRequest => "opening a WinHTTP request",
            Self::QueryDataAvailable => "querying available response data",
            Self::QueryHeaders => "querying WinHTTP headers",
            Self::QueryOption => "querying a WinHTTP option",
            Self::ReadData => "reading WinHTTP response data",
            Self::ReceiveResponse => "receiving a WinHTTP response",
            Self::SendRequest => "sending a WinHTTP request",
            Self::SetOption => "setting a WinHTTP option",
            Self::SetStatusCallback => "registering a WinHTTP status callback",
            Self::SetTimeouts => "setting WinHTTP timeouts",
            Self::WriteData => "writing WinHTTP request data",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WinHttpError {
    code: u32,
    operation: WinHttpOperation,
    secure_failure_flags: Option<u32>,
}

impl WinHttpError {
    pub(crate) const fn new(code: u32, operation: WinHttpOperation) -> Self {
        Self {
            code,
            operation,
            secure_failure_flags: None,
        }
    }

    pub(crate) const fn from_hresult(hresult: i32, operation: WinHttpOperation) -> Self {
        let code = match raw_win32_from_hresult(hresult) {
            Some(code) => code,
            None => hresult.cast_unsigned(),
        };

        Self::new(code, operation)
    }

    pub(crate) const fn code(&self) -> u32 {
        self.code
    }

    pub(crate) const fn operation(&self) -> WinHttpOperation {
        self.operation
    }

    pub(crate) const fn secure_failure_flags(&self) -> Option<u32> {
        self.secure_failure_flags
    }

    pub(crate) const fn with_secure_failure_flags(mut self, flags: u32) -> Self {
        self.secure_failure_flags = Some(flags);
        self
    }

    pub(crate) fn into_http_error(self) -> HttpError {
        let classification = classify(self.code);

        HttpError::other(self, classification.recovery(), classification.label())
    }
}

impl fmt::Display for WinHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed with Win32 error {}", self.operation, self.code)?;

        if let Some(flags) = self.secure_failure_flags {
            write!(f, " (secure-failure flags: 0x{flags:08x})")?;
        }

        Ok(())
    }
}

impl std::error::Error for WinHttpError {}

pub(crate) const fn raw_win32_from_hresult(hresult: i32) -> Option<u32> {
    let bits = hresult.cast_unsigned();

    if bits & 0xffff_0000 == 0x8007_0000 {
        Some(bits & 0x0000_ffff)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ErrorClass {
    Abandoned,
    Connect,
    RequestNever,
    RequestRetry,
    RequestUnknown,
    Timeout,
    Tls,
    TlsRetry,
}

impl ErrorClass {
    const fn label(self) -> ErrorLabel {
        match self {
            Self::Abandoned => error_labels::ABANDONED,
            Self::Connect => error_labels::CONNECT,
            Self::RequestNever | Self::RequestRetry | Self::RequestUnknown => error_labels::REQUEST_WINHTTP,
            Self::Timeout => error_labels::TIMEOUT,
            Self::Tls | Self::TlsRetry => error_labels::TLS,
        }
    }

    const fn recovery(self) -> RecoveryInfo {
        match self {
            Self::Connect | Self::RequestRetry | Self::Timeout | Self::TlsRetry => RecoveryInfo::retry(),
            Self::Abandoned | Self::RequestNever | Self::Tls => RecoveryInfo::never(),
            Self::RequestUnknown => RecoveryInfo::unknown(),
        }
    }
}

#[derive(Clone, Copy)]
struct ErrorMapping {
    code: u32,
    class: ErrorClass,
}

const ERROR_MAPPINGS: &[ErrorMapping] = &[
    mapping(12029, ErrorClass::Connect),      // ERROR_WINHTTP_CANNOT_CONNECT
    mapping(12007, ErrorClass::Connect),      // ERROR_WINHTTP_NAME_NOT_RESOLVED
    mapping(1225, ErrorClass::Connect),       // ERROR_CONNECTION_REFUSED
    mapping(10061, ErrorClass::Connect),      // WSAECONNREFUSED
    mapping(12002, ErrorClass::Timeout),      // ERROR_WINHTTP_TIMEOUT
    mapping(10060, ErrorClass::Timeout),      // WSAETIMEDOUT
    mapping(12017, ErrorClass::Abandoned),    // ERROR_WINHTTP_OPERATION_CANCELLED
    mapping(995, ErrorClass::Abandoned),      // ERROR_OPERATION_ABORTED
    mapping(12012, ErrorClass::Abandoned),    // ERROR_WINHTTP_SHUTDOWN
    mapping(12030, ErrorClass::RequestRetry), // ERROR_WINHTTP_CONNECTION_ERROR
    mapping(12032, ErrorClass::RequestRetry), // ERROR_WINHTTP_RESEND_REQUEST
    mapping(64, ErrorClass::RequestRetry),    // ERROR_NETNAME_DELETED
    mapping(109, ErrorClass::RequestRetry),   // ERROR_BROKEN_PIPE
    mapping(1236, ErrorClass::RequestRetry),  // ERROR_CONNECTION_ABORTED
    mapping(10053, ErrorClass::RequestRetry), // WSAECONNABORTED
    mapping(10054, ErrorClass::RequestRetry), // WSAECONNRESET
    mapping(10058, ErrorClass::RequestRetry), // WSAESHUTDOWN
    mapping(12152, ErrorClass::RequestNever), // ERROR_WINHTTP_INVALID_SERVER_RESPONSE
    mapping(12190, ErrorClass::RequestNever), // ERROR_WINHTTP_HTTP_PROTOCOL_MISMATCH
    mapping(12153, ErrorClass::RequestNever), // ERROR_WINHTTP_INVALID_HEADER
    mapping(12181, ErrorClass::RequestNever), // ERROR_WINHTTP_HEADER_COUNT_EXCEEDED
    mapping(12182, ErrorClass::RequestNever), // ERROR_WINHTTP_HEADER_SIZE_OVERFLOW
    mapping(12183, ErrorClass::RequestNever), // ERROR_WINHTTP_CHUNKED_ENCODING_HEADER_SIZE_OVERFLOW
    mapping(12184, ErrorClass::RequestNever), // ERROR_WINHTTP_RESPONSE_DRAIN_OVERFLOW
    mapping(12037, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_CERT_DATE_INVALID
    mapping(12038, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_CERT_CN_INVALID
    mapping(12044, ErrorClass::Tls),          // ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED
    mapping(12045, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_INVALID_CA
    mapping(12057, ErrorClass::TlsRetry),     // ERROR_WINHTTP_SECURE_CERT_REV_FAILED
    mapping(12157, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_CHANNEL_ERROR
    mapping(12169, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_INVALID_CERT
    mapping(12170, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_CERT_REVOKED
    mapping(12175, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_FAILURE
    mapping(12179, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE
    mapping(12185, ErrorClass::Tls),          // ERROR_WINHTTP_CLIENT_CERT_NO_PRIVATE_KEY
    mapping(12186, ErrorClass::Tls),          // ERROR_WINHTTP_CLIENT_CERT_NO_ACCESS_PRIVATE_KEY
    mapping(12187, ErrorClass::Tls),          // ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED_PROXY
    mapping(12188, ErrorClass::Tls),          // ERROR_WINHTTP_SECURE_FAILURE_PROXY
];

const fn mapping(code: u32, class: ErrorClass) -> ErrorMapping {
    ErrorMapping { code, class }
}

fn classify(code: u32) -> ErrorClass {
    ERROR_MAPPINGS
        .iter()
        .find(|mapping| mapping.code == code)
        .map_or(ErrorClass::RequestUnknown, |mapping| mapping.class)
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use ohno::Labeled as _;
    use recoverable::{Recovery, RecoveryInfo};

    use super::{ERROR_MAPPINGS, ErrorClass, WinHttpError, WinHttpOperation, raw_win32_from_hresult};

    #[test]
    fn extracts_raw_win32_code_from_hresult() {
        assert_eq!(raw_win32_from_hresult(0x8007_2efd_u32.cast_signed()), Some(12029));
        assert_eq!(raw_win32_from_hresult(0x8000_4005_u32.cast_signed()), None);
    }

    #[test]
    fn hresult_constructor_preserves_non_win32_hresult_bits() {
        let error = WinHttpError::from_hresult(0x8000_4005_u32.cast_signed(), WinHttpOperation::SetOption);

        assert_eq!(error.code(), 0x8000_4005);
        assert_eq!(error.operation(), WinHttpOperation::SetOption);
    }

    #[test]
    fn secure_failure_flags_are_retained() {
        let error = WinHttpError::new(12175, WinHttpOperation::SendRequest).with_secure_failure_flags(0x20);

        assert_eq!(error.secure_failure_flags(), Some(0x20));
        assert!(error.to_string().contains("0x00000020"));
    }

    #[test]
    fn maps_every_documented_error() {
        for mapping in ERROR_MAPPINGS {
            let error = WinHttpError::new(mapping.code, WinHttpOperation::SendRequest).into_http_error();

            assert_eq!(error.label(), mapping.class.label().as_str(), "code {}", mapping.code);
            assert_eq!(error.recovery(), mapping.class.recovery(), "code {}", mapping.code);
            assert!(
                error.source().is_some_and(|source| source.downcast_ref::<WinHttpError>().is_some()),
                "code {}",
                mapping.code
            );
        }

        let unknown = WinHttpError::new(12004, WinHttpOperation::SendRequest).into_http_error();
        assert_eq!(unknown.label(), ErrorClass::RequestUnknown.label().as_str());
        assert_eq!(unknown.recovery(), RecoveryInfo::unknown());
    }

    #[test]
    fn representative_error_families_have_independent_contract_expectations() {
        let cases = [
            (12029, "connect", RecoveryInfo::retry()),
            (12002, "timeout", RecoveryInfo::retry()),
            (10060, "timeout", RecoveryInfo::retry()),
            (12017, "abandoned", RecoveryInfo::never()),
            (12030, "request_winhttp", RecoveryInfo::retry()),
            (12152, "request_winhttp", RecoveryInfo::never()),
            (12175, "tls", RecoveryInfo::never()),
            (12057, "tls", RecoveryInfo::retry()),
            (12004, "request_winhttp", RecoveryInfo::unknown()),
        ];

        for (code, expected_label, expected_recovery) in cases {
            let error = WinHttpError::new(code, WinHttpOperation::SendRequest).into_http_error();

            assert_eq!(error.label(), expected_label, "code {code}");
            assert_eq!(error.recovery(), expected_recovery, "code {code}");
        }
    }
}
