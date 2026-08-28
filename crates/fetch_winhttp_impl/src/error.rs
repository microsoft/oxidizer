// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;

use http_extensions::HttpError;
use ohno::ErrorLabel;
use recoverable::RecoveryInfo;
use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_CONNECTION_ABORTED, ERROR_CONNECTION_REFUSED, ERROR_NETNAME_DELETED, ERROR_OPERATION_ABORTED,
};
use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_CHUNKED_ENCODING_HEADER_SIZE_OVERFLOW, ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED,
    ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED_PROXY, ERROR_WINHTTP_CLIENT_CERT_NO_ACCESS_PRIVATE_KEY, ERROR_WINHTTP_CLIENT_CERT_NO_PRIVATE_KEY,
    ERROR_WINHTTP_CONNECTION_ERROR, ERROR_WINHTTP_HEADER_COUNT_EXCEEDED, ERROR_WINHTTP_HEADER_SIZE_OVERFLOW,
    ERROR_WINHTTP_HTTP_PROTOCOL_MISMATCH, ERROR_WINHTTP_INVALID_HEADER, ERROR_WINHTTP_INVALID_SERVER_RESPONSE,
    ERROR_WINHTTP_NAME_NOT_RESOLVED, ERROR_WINHTTP_OPERATION_CANCELLED, ERROR_WINHTTP_RESEND_REQUEST,
    ERROR_WINHTTP_RESPONSE_DRAIN_OVERFLOW, ERROR_WINHTTP_SECURE_CERT_CN_INVALID, ERROR_WINHTTP_SECURE_CERT_DATE_INVALID,
    ERROR_WINHTTP_SECURE_CERT_REV_FAILED, ERROR_WINHTTP_SECURE_CERT_REVOKED, ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE,
    ERROR_WINHTTP_SECURE_CHANNEL_ERROR, ERROR_WINHTTP_SECURE_FAILURE, ERROR_WINHTTP_SECURE_FAILURE_PROXY, ERROR_WINHTTP_SECURE_INVALID_CA,
    ERROR_WINHTTP_SECURE_INVALID_CERT, ERROR_WINHTTP_SHUTDOWN, ERROR_WINHTTP_TIMEOUT,
};
use windows::Win32::Networking::WinSock::{WSAECONNABORTED, WSAECONNREFUSED, WSAECONNRESET, WSAESHUTDOWN, WSAETIMEDOUT};

use crate::error_labels;
use crate::query::QueryError;

/// Result type for direct WinHTTP binding operations.
pub(crate) type Result<T> = std::result::Result<T, WinHttpError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Identifies the native operation associated with a WinHTTP failure.
///
/// This context makes the preserved source error actionable without affecting
/// recovery classification, which is determined from the operating-system code.
pub(crate) enum WinHttpOperation {
    CloseHandle,
    Connect,
    Open,
    OpenRequest,
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
/// Retains native failure details until they become a transport error.
///
/// Conversion to [`HttpError`] preserves this value as the source while adding a
/// stable public label and [`RecoveryInfo`] derived from the Win32 error code.
/// The label is contractual; which recovery guidance a given code receives is
/// not, and may change as classification is refined (design.md section 7.1).
/// Secure-failure flags are best-effort certificate diagnostics captured from a
/// separate callback.
///
/// Classification depends on the request-error code alone, never on the order in
/// which the secure-failure and request-error callbacks arrive; only secure
/// diagnostics observed in time are attached to the source.
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

    #[cfg(test)]
    pub(crate) const fn operation(&self) -> WinHttpOperation {
        self.operation
    }

    #[cfg(test)]
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

    if bits & HRESULT_WIN32_PREFIX_MASK == HRESULT_WIN32_PREFIX {
        Some(bits & HRESULT_CODE_MASK)
    } else {
        None
    }
}

// HRESULT_FROM_WIN32 encodes the Win32 facility and low 16-bit error code:
// https://learn.microsoft.com/windows/win32/api/winerror/nf-winerror-hresult_from_win32
const HRESULT_WIN32_PREFIX_MASK: u32 = 0xffff_0000;
const HRESULT_WIN32_PREFIX: u32 = 0x8007_0000;
const HRESULT_CODE_MASK: u32 = 0x0000_ffff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Maps native failures to stable labels and recovery guidance.
///
/// Each class collapses related Win32 codes into a public error label and the
/// [`RecoveryInfo`] consumed by retry and breaker layers. Transient connection,
/// timeout, request, or revocation-check failures may be retryable; deterministic
/// protocol, cancellation, and certificate failures are not, while unknown codes
/// retain unknown recovery guidance. That split is descriptive rather than
/// contractual and may be refined without a breaking change.
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
/// Associates one recognized native code with its transport error semantics.
///
/// The table keeps code recognition separate from the label and recovery policy
/// centralized in [`ErrorClass`]. Which codes appear here is not contractual
/// (design.md section 7): the conditions each label covers are the promise, and
/// a code absent from the table is classified as unknown.
struct ErrorMapping {
    code: u32,
    class: ErrorClass,
}

const ERROR_MAPPINGS: &[ErrorMapping] = &[
    mapping(ERROR_WINHTTP_CANNOT_CONNECT, ErrorClass::Connect),
    mapping(ERROR_WINHTTP_NAME_NOT_RESOLVED, ErrorClass::Connect),
    mapping(ERROR_CONNECTION_REFUSED.0, ErrorClass::Connect),
    mapping(WSAECONNREFUSED.0.cast_unsigned(), ErrorClass::Connect),
    mapping(ERROR_WINHTTP_TIMEOUT, ErrorClass::Timeout),
    mapping(WSAETIMEDOUT.0.cast_unsigned(), ErrorClass::Timeout),
    mapping(ERROR_WINHTTP_OPERATION_CANCELLED, ErrorClass::Abandoned),
    mapping(ERROR_OPERATION_ABORTED.0, ErrorClass::Abandoned),
    mapping(ERROR_WINHTTP_SHUTDOWN, ErrorClass::Abandoned),
    mapping(ERROR_WINHTTP_CONNECTION_ERROR, ErrorClass::RequestRetry),
    mapping(ERROR_WINHTTP_RESEND_REQUEST, ErrorClass::RequestRetry),
    mapping(ERROR_NETNAME_DELETED.0, ErrorClass::RequestRetry),
    mapping(ERROR_BROKEN_PIPE.0, ErrorClass::RequestRetry),
    mapping(ERROR_CONNECTION_ABORTED.0, ErrorClass::RequestRetry),
    mapping(WSAECONNABORTED.0.cast_unsigned(), ErrorClass::RequestRetry),
    mapping(WSAECONNRESET.0.cast_unsigned(), ErrorClass::RequestRetry),
    mapping(WSAESHUTDOWN.0.cast_unsigned(), ErrorClass::RequestRetry),
    mapping(ERROR_WINHTTP_INVALID_SERVER_RESPONSE, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_HTTP_PROTOCOL_MISMATCH, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_INVALID_HEADER, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_HEADER_COUNT_EXCEEDED, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_HEADER_SIZE_OVERFLOW, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_CHUNKED_ENCODING_HEADER_SIZE_OVERFLOW, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_RESPONSE_DRAIN_OVERFLOW, ErrorClass::RequestNever),
    mapping(ERROR_WINHTTP_SECURE_CERT_DATE_INVALID, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_CERT_CN_INVALID, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_INVALID_CA, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_CERT_REV_FAILED, ErrorClass::TlsRetry),
    mapping(ERROR_WINHTTP_SECURE_CHANNEL_ERROR, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_INVALID_CERT, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_CERT_REVOKED, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_FAILURE, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_CERT_WRONG_USAGE, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_CLIENT_CERT_NO_PRIVATE_KEY, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_CLIENT_CERT_NO_ACCESS_PRIVATE_KEY, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_CLIENT_AUTH_CERT_NEEDED_PROXY, ErrorClass::Tls),
    mapping(ERROR_WINHTTP_SECURE_FAILURE_PROXY, ErrorClass::Tls),
];

// Only ever evaluated while building ERROR_MAPPINGS, which is a constant, so
// this never executes at run time and can carry no run-time coverage.
#[cfg_attr(coverage_nightly, coverage(off))]
const fn mapping(code: u32, class: ErrorClass) -> ErrorMapping {
    ErrorMapping { code, class }
}

fn classify(code: u32) -> ErrorClass {
    ERROR_MAPPINGS
        .iter()
        .find(|mapping| mapping.code == code)
        .map_or(ErrorClass::RequestUnknown, |mapping| mapping.class)
}

/// Reports a request this transport rejects itself rather than `WinHTTP`.
///
/// This is the `invalid_request` row of the error-surface table in design.md
/// section 7: an unusable HTTP version, an unusable target, or request body
/// framing the transport cannot honor. The same request would be rejected
/// identically on every attempt, so design.md section 7.1 classifies it as
/// deterministic rather than unknown.
///
/// A rejection decided from request metadata happens before any `WinHTTP`
/// call, but one decided from a body frame is reached only once that frame is
/// polled, after the headers and every preceding data frame have been sent.
/// This label therefore does not imply the request had no remote effect.
pub(crate) fn invalid_request(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), error_labels::INVALID_REQUEST)
}

/// Reports response metadata `WinHTTP` returned that the transport cannot use.
///
/// This is the `request_winhttp` row of design.md section 7 applied to a
/// native call that itself succeeded but produced unusable output: an
/// out-of-range status code, a malformed header or trailer block, or a value
/// conversion failure. Such output reflects a stable server or configuration
/// problem, which design.md section 7.1 lists as non-retryable.
pub(crate) fn invalid_response(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), error_labels::REQUEST_WINHTTP)
}

/// Reports a callback sequence that contradicts the `WinHTTP` asynchronous
/// model.
///
/// Unexpected or undecodable completions, and a completion channel that
/// disconnects without delivering a result, mean the observed sequence
/// violates the documented model (implementation.md section 2). The caller
/// cannot distinguish this from any other unusable transport response, so it
/// carries exactly the [`invalid_response`] contract. The separate name keeps
/// call sites explicit about which condition they detected and gives the two
/// categories one place to diverge should that ever be warranted.
pub(crate) fn callback_protocol_error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    invalid_response(error)
}

/// Routes a synchronous query failure into the matching error category.
///
/// [`QueryError`] keeps an operating-system failure separate from malformed
/// data returned by a call that succeeded, because design.md section 7 gives
/// the two different labels and recovery classifications: the former is
/// classified from its Win32 code and may be retryable, while the latter is
/// always a non-recoverable `request_winhttp` invalid response.
pub(crate) fn query_error(error: QueryError) -> HttpError {
    match error {
        QueryError::WinHttp(error) => error.into_http_error(),
        QueryError::Conversion(error) => invalid_response(error),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error as _;
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use ohno::Labeled as _;
    use recoverable::{Recovery, RecoveryInfo};
    use static_assertions::assert_impl_all;
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::Networking::WinHttp::{
        ERROR_WINHTTP_CANNOT_CONNECT, ERROR_WINHTTP_CONNECTION_ERROR, ERROR_WINHTTP_INTERNAL_ERROR, ERROR_WINHTTP_INVALID_SERVER_RESPONSE,
        ERROR_WINHTTP_OPERATION_CANCELLED, ERROR_WINHTTP_SECURE_CERT_REV_FAILED, ERROR_WINHTTP_SECURE_FAILURE, ERROR_WINHTTP_TIMEOUT,
    };
    use windows::Win32::Networking::WinSock::WSAETIMEDOUT;

    use super::{
        ERROR_MAPPINGS, ErrorClass, ErrorMapping, HRESULT_WIN32_PREFIX, Result, WinHttpError, WinHttpOperation, raw_win32_from_hresult,
    };

    assert_impl_all!(Result<()>: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WinHttpOperation: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(WinHttpError: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ErrorClass: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ErrorMapping: UnwindSafe, RefUnwindSafe);

    #[test]
    fn extracts_raw_win32_code_from_hresult() {
        let hresult = (HRESULT_WIN32_PREFIX | ERROR_WINHTTP_CANNOT_CONNECT).cast_signed();
        assert_eq!(raw_win32_from_hresult(hresult), Some(ERROR_WINHTTP_CANNOT_CONNECT));
        assert_eq!(raw_win32_from_hresult(E_FAIL.0), None);
    }

    #[test]
    fn hresult_constructor_preserves_non_win32_hresult_bits() {
        let error = WinHttpError::from_hresult(E_FAIL.0, WinHttpOperation::SetOption);

        assert_eq!(error.code(), E_FAIL.0.cast_unsigned());
        assert_eq!(error.operation(), WinHttpOperation::SetOption);
    }

    #[test]
    fn secure_failure_flags_are_retained() {
        let error = WinHttpError::new(ERROR_WINHTTP_SECURE_FAILURE, WinHttpOperation::SendRequest).with_secure_failure_flags(0x20);

        assert_eq!(error.secure_failure_flags(), Some(0x20));
        assert!(error.to_string().contains("0x00000020"));
    }

    #[test]
    fn every_mapping_reaches_the_http_error_through_its_class() {
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

        let unknown = WinHttpError::new(ERROR_WINHTTP_INTERNAL_ERROR, WinHttpOperation::SendRequest).into_http_error();
        assert_eq!(unknown.label(), ErrorClass::RequestUnknown.label().as_str());
        assert_eq!(unknown.recovery(), RecoveryInfo::unknown());
    }

    #[test]
    fn representative_error_families_have_independent_contract_expectations() {
        let cases = [
            (ERROR_WINHTTP_CANNOT_CONNECT, "connect", RecoveryInfo::retry()),
            (ERROR_WINHTTP_TIMEOUT, "timeout", RecoveryInfo::retry()),
            (WSAETIMEDOUT.0.cast_unsigned(), "timeout", RecoveryInfo::retry()),
            (ERROR_WINHTTP_OPERATION_CANCELLED, "abandoned", RecoveryInfo::never()),
            (ERROR_WINHTTP_CONNECTION_ERROR, "request_winhttp", RecoveryInfo::retry()),
            (ERROR_WINHTTP_INVALID_SERVER_RESPONSE, "request_winhttp", RecoveryInfo::never()),
            (ERROR_WINHTTP_SECURE_FAILURE, "tls", RecoveryInfo::never()),
            (ERROR_WINHTTP_SECURE_CERT_REV_FAILED, "tls", RecoveryInfo::retry()),
            (ERROR_WINHTTP_INTERNAL_ERROR, "request_winhttp", RecoveryInfo::unknown()),
        ];

        for (code, expected_label, expected_recovery) in cases {
            let error = WinHttpError::new(code, WinHttpOperation::SendRequest).into_http_error();

            assert_eq!(error.label(), expected_label, "code {code}");
            assert_eq!(error.recovery(), expected_recovery, "code {code}");
        }
    }

    #[test]
    fn every_operation_describes_itself_distinctly_in_the_error_message() {
        let operations = [
            WinHttpOperation::CloseHandle,
            WinHttpOperation::Connect,
            WinHttpOperation::Open,
            WinHttpOperation::OpenRequest,
            WinHttpOperation::QueryHeaders,
            WinHttpOperation::QueryOption,
            WinHttpOperation::ReadData,
            WinHttpOperation::ReceiveResponse,
            WinHttpOperation::SendRequest,
            WinHttpOperation::SetOption,
            WinHttpOperation::SetStatusCallback,
            WinHttpOperation::SetTimeouts,
            WinHttpOperation::WriteData,
        ];

        let mut descriptions = Vec::with_capacity(operations.len());

        for operation in operations {
            let description = operation.to_string();

            assert!(!description.is_empty(), "{operation:?} has no description");

            // The description identifies the failing call within the error
            // message, so it must reach the rendered error verbatim.
            let message = WinHttpError::new(12002, operation).to_string();
            assert_eq!(message, format!("{description} failed with Win32 error 12002"));

            descriptions.push(description);
        }

        let mut unique = descriptions.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), descriptions.len(), "operation descriptions must be distinct");
    }
}
