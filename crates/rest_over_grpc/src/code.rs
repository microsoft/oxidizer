// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Code`] gRPC status code and HTTP status mapping helpers.

use core::fmt;
use std::error::Error;

use http::StatusCode;

/// A canonical gRPC status code, mirroring
/// [`google.rpc.Code`](https://github.com/googleapis/googleapis/blob/master/google/rpc/code.proto).
///
/// The integer discriminants are wire-compatible with `google.rpc.Code`, so
/// [`Code::from_i32`] / [`Code::as_i32`] round-trip values exchanged over the
/// gRPC wire.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::handling::Code;
///
/// let code = Code::InvalidArgument;
/// let wire: i32 = code.into();
/// assert_eq!(wire, 3);
/// assert_eq!(Code::from_i32(wire), Some(code));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Code {
    /// Not an error; returned on success (`0`).
    Ok = 0,
    /// The operation was cancelled, typically by the caller (`1`).
    Cancelled = 1,
    /// Unknown error (`2`).
    Unknown = 2,
    /// The client specified an invalid argument (`3`).
    InvalidArgument = 3,
    /// The deadline expired before the operation could complete (`4`).
    DeadlineExceeded = 4,
    /// Some requested entity was not found (`5`).
    NotFound = 5,
    /// The entity that a client attempted to create already exists (`6`).
    AlreadyExists = 6,
    /// The caller does not have permission to execute the operation (`7`).
    PermissionDenied = 7,
    /// Some resource has been exhausted (`8`).
    ResourceExhausted = 8,
    /// The operation was rejected because the system is not in a state required
    /// for its execution (`9`).
    FailedPrecondition = 9,
    /// The operation was aborted, typically due to a concurrency issue (`10`).
    Aborted = 10,
    /// The operation was attempted past the valid range (`11`).
    OutOfRange = 11,
    /// The operation is not implemented or not supported (`12`).
    Unimplemented = 12,
    /// Internal error (`13`).
    Internal = 13,
    /// The service is currently unavailable (`14`).
    Unavailable = 14,
    /// Unrecoverable data loss or corruption (`15`).
    DataLoss = 15,
    /// The request does not have valid authentication credentials (`16`).
    Unauthenticated = 16,
}

impl Code {
    /// Returns the wire integer discriminant of this code.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::Code;
    ///
    /// assert_eq!(Code::NotFound.as_i32(), 5);
    /// ```
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Converts a wire integer discriminant into a [`Code`].
    ///
    /// Returns [`None`] if `value` is not a recognized `google.rpc.Code`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::Code;
    ///
    /// assert_eq!(Code::from_i32(5), Some(Code::NotFound));
    /// assert_eq!(Code::from_i32(999), None);
    /// ```
    #[must_use]
    pub const fn from_i32(value: i32) -> Option<Self> {
        let code = match value {
            0 => Self::Ok,
            1 => Self::Cancelled,
            2 => Self::Unknown,
            3 => Self::InvalidArgument,
            4 => Self::DeadlineExceeded,
            5 => Self::NotFound,
            6 => Self::AlreadyExists,
            7 => Self::PermissionDenied,
            8 => Self::ResourceExhausted,
            9 => Self::FailedPrecondition,
            10 => Self::Aborted,
            11 => Self::OutOfRange,
            12 => Self::Unimplemented,
            13 => Self::Internal,
            14 => Self::Unavailable,
            15 => Self::DataLoss,
            16 => Self::Unauthenticated,
            _ => return None,
        };
        Some(code)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Ok => "OK",
            Self::Cancelled => "CANCELLED",
            Self::Unknown => "UNKNOWN",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::NotFound => "NOT_FOUND",
            Self::AlreadyExists => "ALREADY_EXISTS",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::ResourceExhausted => "RESOURCE_EXHAUSTED",
            Self::FailedPrecondition => "FAILED_PRECONDITION",
            Self::Aborted => "ABORTED",
            Self::OutOfRange => "OUT_OF_RANGE",
            Self::Unimplemented => "UNIMPLEMENTED",
            Self::Internal => "INTERNAL",
            Self::Unavailable => "UNAVAILABLE",
            Self::DataLoss => "DATA_LOSS",
            Self::Unauthenticated => "UNAUTHENTICATED",
        };
        f.write_str(name)
    }
}

impl From<Code> for i32 {
    fn from(code: Code) -> Self {
        code.as_i32()
    }
}

/// The error returned when converting an `i32` that is not a recognized
/// [`Code`] discriminant, via `Code`'s [`TryFrom`] implementation.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::handling::Code;
///
/// let error = Code::try_from(999).expect_err("999 is not a canonical code");
/// assert_eq!(error.value(), 999);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownCode(i32);

impl UnknownCode {
    /// The unrecognized wire value that could not be converted.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::Code;
    ///
    /// let error = Code::try_from(-1).expect_err("-1 is not a canonical code");
    /// assert_eq!(error.value(), -1);
    /// ```
    #[must_use]
    pub const fn value(self) -> i32 {
        self.0
    }
}

impl fmt::Display for UnknownCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown gRPC status code: {}", self.0)
    }
}

impl Error for UnknownCode {}

impl TryFrom<i32> for Code {
    type Error = UnknownCode;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::from_i32(value).ok_or(UnknownCode(value))
    }
}

impl Code {
    /// Maps this gRPC code to the HTTP [`StatusCode`] a REST gateway returns for
    /// it.
    ///
    /// This mirrors gRPC-Gateway's `HTTPStatusFromCode`. Several distinct gRPC
    /// codes intentionally collapse onto the same HTTP status (for example both
    /// [`Code::InvalidArgument`] and [`Code::OutOfRange`] map to `400`), so the
    /// mapping is not injective and does not perfectly round-trip through
    /// [`Code::from_http_status`].
    ///
    /// | gRPC code | HTTP status |
    /// |---|---|
    /// | `Ok` | `200 OK` |
    /// | `Cancelled` | `499 Client Closed Request` |
    /// | `Unknown`, `Internal`, `DataLoss` | `500 Internal Server Error` |
    /// | `InvalidArgument`, `FailedPrecondition`, `OutOfRange` | `400 Bad Request` |
    /// | `DeadlineExceeded` | `504 Gateway Timeout` |
    /// | `NotFound` | `404 Not Found` |
    /// | `AlreadyExists`, `Aborted` | `409 Conflict` |
    /// | `PermissionDenied` | `403 Forbidden` |
    /// | `Unauthenticated` | `401 Unauthorized` |
    /// | `ResourceExhausted` | `429 Too Many Requests` |
    /// | `Unimplemented` | `501 Not Implemented` |
    /// | `Unavailable` | `503 Service Unavailable` |
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::handling::Code;
    ///
    /// assert_eq!(Code::NotFound.to_http_status(), StatusCode::NOT_FOUND);
    /// assert_eq!(Code::OutOfRange.to_http_status(), StatusCode::BAD_REQUEST);
    /// ```
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panic")]
    pub fn to_http_status(self) -> StatusCode {
        match self {
            Self::Ok => StatusCode::OK,
            // 499 "Client Closed Request" is non-standard but is what gateways use.
            Self::Cancelled => StatusCode::from_u16(499).expect("499 is a valid HTTP status code"),
            Self::Unknown | Self::Internal | Self::DataLoss => StatusCode::INTERNAL_SERVER_ERROR,
            Self::InvalidArgument | Self::FailedPrecondition | Self::OutOfRange => StatusCode::BAD_REQUEST,
            Self::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::AlreadyExists | Self::Aborted => StatusCode::CONFLICT,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
            Self::Unimplemented => StatusCode::NOT_IMPLEMENTED,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Maps an HTTP [`StatusCode`] to a gRPC code.
    ///
    /// This mirrors the gRPC core `http2 status → grpc status` table used when a
    /// proxy translates a non-`200` HTTP response into a gRPC status. Any `2xx`
    /// status maps to [`Code::Ok`]; unrecognized statuses map to [`Code::Unknown`].
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use rest_over_grpc::handling::Code;
    ///
    /// assert_eq!(Code::from_http_status(StatusCode::NO_CONTENT), Code::Ok);
    /// assert_eq!(
    ///     Code::from_http_status(StatusCode::FORBIDDEN),
    ///     Code::PermissionDenied
    /// );
    /// ```
    #[must_use]
    pub fn from_http_status(status: StatusCode) -> Self {
        match status.as_u16() {
            200..=299 => Self::Ok,
            400 => Self::Internal,
            401 => Self::Unauthenticated,
            403 => Self::PermissionDenied,
            404 => Self::Unimplemented,
            429 | 502 | 503 | 504 => Self::Unavailable,
            _ => Self::Unknown,
        }
    }
}
