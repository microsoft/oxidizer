// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Code`] gRPC status code and HTTP status mapping helpers.

use core::fmt;

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
/// use rest_over_grpc::Code;
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
    /// use rest_over_grpc::Code;
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
    /// use rest_over_grpc::Code;
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
    /// Renders the canonical `google.rpc.Code` name, e.g. `NOT_FOUND`.
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
    /// Returns the wire integer discriminant (see [`Code::as_i32`]).
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
/// use rest_over_grpc::Code;
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
    /// use rest_over_grpc::Code;
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

impl std::error::Error for UnknownCode {}

impl TryFrom<i32> for Code {
    type Error = UnknownCode;

    /// Converts a wire discriminant into a [`Code`] (see [`Code::from_i32`]),
    /// returning [`UnknownCode`] for unrecognized values.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::Code;
    ///
    /// assert_eq!(Code::try_from(5), Ok(Code::NotFound));
    /// assert!(Code::try_from(999).is_err());
    /// ```
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::from_i32(value).ok_or(UnknownCode(value))
    }
}

/// Maps a gRPC [`Code`] to the HTTP [`StatusCode`] a REST gateway returns for
/// it.
///
/// This mirrors gRPC-Gateway's `HTTPStatusFromCode`. Several distinct gRPC
/// codes intentionally collapse onto the same HTTP status (for example both
/// [`Code::InvalidArgument`] and [`Code::OutOfRange`] map to `400`), so the
/// mapping is not injective and does not perfectly round-trip through
/// [`map_http_to_code`].
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use rest_over_grpc::{Code, map_code_to_http};
///
/// assert_eq!(map_code_to_http(Code::NotFound), StatusCode::NOT_FOUND);
/// assert_eq!(map_code_to_http(Code::OutOfRange), StatusCode::BAD_REQUEST);
/// ```
#[must_use]
pub fn map_code_to_http(code: Code) -> StatusCode {
    match code {
        Code::Ok => StatusCode::OK,
        // 499 "Client Closed Request" is non-standard but is what gateways use;
        // it is always a valid status code, so the fallback is unreachable.
        Code::Cancelled => StatusCode::from_u16(499).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Code::Unknown | Code::Internal | Code::DataLoss => StatusCode::INTERNAL_SERVER_ERROR,
        Code::InvalidArgument | Code::FailedPrecondition | Code::OutOfRange => StatusCode::BAD_REQUEST,
        Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::AlreadyExists | Code::Aborted => StatusCode::CONFLICT,
        Code::PermissionDenied => StatusCode::FORBIDDEN,
        Code::Unauthenticated => StatusCode::UNAUTHORIZED,
        Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        Code::Unimplemented => StatusCode::NOT_IMPLEMENTED,
        Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Maps an HTTP [`StatusCode`] to a gRPC [`Code`].
///
/// This mirrors the gRPC core `http2 status → grpc status` table used when a
/// proxy translates a non-`200` HTTP response into a gRPC status. Any `2xx`
/// status maps to [`Code::Ok`]; unrecognized statuses map to [`Code::Unknown`].
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use rest_over_grpc::{Code, map_http_to_code};
///
/// assert_eq!(map_http_to_code(StatusCode::NO_CONTENT), Code::Ok);
/// assert_eq!(
///     map_http_to_code(StatusCode::FORBIDDEN),
///     Code::PermissionDenied
/// );
/// ```
#[must_use]
pub fn map_http_to_code(status: StatusCode) -> Code {
    match status.as_u16() {
        200..=299 => Code::Ok,
        400 => Code::Internal,
        401 => Code::Unauthenticated,
        403 => Code::PermissionDenied,
        404 => Code::Unimplemented,
        429 | 502 | 503 | 504 => Code::Unavailable,
        _ => Code::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i32_round_trips() {
        for value in 0..=16 {
            let code = Code::from_i32(value).expect("0..=16 are valid codes");
            assert_eq!(code.as_i32(), value);
        }
        assert_eq!(Code::from_i32(17), None);
        assert_eq!(Code::from_i32(-1), None);
    }

    #[test]
    fn i32_from_code_matches_as_i32() {
        assert_eq!(i32::from(Code::NotFound), 5);
        assert_eq!(i32::from(Code::Ok), 0);
    }

    #[test]
    fn display_renders_canonical_names() {
        assert_eq!(Code::Ok.to_string(), "OK");
        assert_eq!(Code::InvalidArgument.to_string(), "INVALID_ARGUMENT");
        assert_eq!(Code::Unauthenticated.to_string(), "UNAUTHENTICATED");
        // Every code renders a non-empty, upper-case canonical name.
        for value in 0..=16 {
            let code = Code::from_i32(value).expect("0..=16 are valid codes");
            let name = code.to_string();
            assert!(!name.is_empty());
            assert_eq!(name, name.to_uppercase());
        }
    }

    #[test]
    fn try_from_i32_round_trips_and_reports_unknown() {
        assert_eq!(Code::try_from(5), Ok(Code::NotFound));
        let error = Code::try_from(999).expect_err("999 is not a canonical code");
        assert_eq!(error.value(), 999);
        assert_eq!(error, Code::try_from(999).unwrap_err());
        assert_eq!(error.to_string(), "unknown gRPC status code: 999");
    }

    #[test]
    fn forward_mapping_known_values() {
        assert_eq!(map_code_to_http(Code::Ok), StatusCode::OK);
        assert_eq!(map_code_to_http(Code::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(map_code_to_http(Code::AlreadyExists), StatusCode::CONFLICT);
        assert_eq!(map_code_to_http(Code::Aborted), StatusCode::CONFLICT);
        assert_eq!(map_code_to_http(Code::PermissionDenied), StatusCode::FORBIDDEN);
        assert_eq!(map_code_to_http(Code::Unauthenticated), StatusCode::UNAUTHORIZED);
        assert_eq!(map_code_to_http(Code::ResourceExhausted), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(map_code_to_http(Code::Unimplemented), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(map_code_to_http(Code::DeadlineExceeded), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(map_code_to_http(Code::Cancelled).as_u16(), 499);
    }

    #[test]
    fn forward_mapping_covers_remaining_arms() {
        assert_eq!(map_code_to_http(Code::Unknown), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(map_code_to_http(Code::Internal), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(map_code_to_http(Code::DataLoss), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(map_code_to_http(Code::InvalidArgument), StatusCode::BAD_REQUEST);
        assert_eq!(map_code_to_http(Code::FailedPrecondition), StatusCode::BAD_REQUEST);
        assert_eq!(map_code_to_http(Code::OutOfRange), StatusCode::BAD_REQUEST);
        assert_eq!(map_code_to_http(Code::Unavailable), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn reverse_mapping_known_values() {
        assert_eq!(map_http_to_code(StatusCode::OK), Code::Ok);
        assert_eq!(map_http_to_code(StatusCode::CREATED), Code::Ok);
        assert_eq!(map_http_to_code(StatusCode::BAD_REQUEST), Code::Internal);
        assert_eq!(map_http_to_code(StatusCode::UNAUTHORIZED), Code::Unauthenticated);
        assert_eq!(map_http_to_code(StatusCode::FORBIDDEN), Code::PermissionDenied);
        assert_eq!(map_http_to_code(StatusCode::NOT_FOUND), Code::Unimplemented);
        assert_eq!(map_http_to_code(StatusCode::SERVICE_UNAVAILABLE), Code::Unavailable);
        assert_eq!(map_http_to_code(StatusCode::INTERNAL_SERVER_ERROR), Code::Unknown);
    }
}
