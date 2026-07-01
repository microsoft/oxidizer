// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Status`] gRPC status message type.

use core::fmt;

use crate::code::Code;

/// A gRPC status: a [`Code`] plus a human-readable message.
///
/// Generated service-trait methods return `Result<Response, Status>`; a
/// generated dispatcher maps the [`Code`] to an HTTP status via
/// [`map_code_to_http`](crate::map_code_to_http) and renders the status as a
/// JSON body.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::{Code, Status};
///
/// let invalid = Status::invalid_argument("shelf id must be numeric");
/// assert_eq!(invalid.code(), Code::InvalidArgument);
/// assert_eq!(invalid.message(), "shelf id must be numeric");
///
/// let missing = Status::not_found("shelf 7");
/// assert_eq!(missing.code(), Code::NotFound);
///
/// let internal = Status::internal("database unavailable");
/// assert_eq!(internal.code(), Code::Internal);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Status {
    code: Code,
    message: String,
}

impl Status {
    /// Creates a status with the given `code` and `message`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Code, Status};
    ///
    /// let status = Status::new(Code::NotFound, "shelf 7 does not exist");
    /// assert_eq!(status.code(), Code::NotFound);
    /// assert_eq!(status.message(), "shelf 7 does not exist");
    /// ```
    #[must_use]
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Creates an [`Code::InvalidArgument`] status.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Code, Status};
    ///
    /// let status = Status::invalid_argument("bad shelf id");
    /// assert_eq!(status.code(), Code::InvalidArgument);
    /// assert_eq!(status.message(), "bad shelf id");
    /// ```
    #[must_use]
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(Code::InvalidArgument, message)
    }

    /// Creates a [`Code::NotFound`] status.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Code, Status};
    ///
    /// let status = Status::not_found("shelf 7");
    /// assert_eq!(status.code(), Code::NotFound);
    /// ```
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(Code::NotFound, message)
    }

    /// Creates a [`Code::Internal`] status.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Code, Status};
    ///
    /// let status = Status::internal("storage failed");
    /// assert_eq!(status.code(), Code::Internal);
    /// ```
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Code::Internal, message)
    }

    /// The gRPC status code.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::{Code, Status};
    ///
    /// let status = Status::not_found("missing");
    /// assert_eq!(status.code(), Code::NotFound);
    /// ```
    #[must_use]
    pub const fn code(&self) -> Code {
        self.code
    }

    /// The human-readable status message.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::Status;
    ///
    /// let status = Status::invalid_argument("bad request");
    /// assert_eq!(status.message(), "bad request");
    /// ```
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for Status {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_accessors() {
        let s = Status::invalid_argument("bad shelf id");
        assert_eq!(s.code(), Code::InvalidArgument);
        assert_eq!(s.message(), "bad shelf id");
        assert!(s.to_string().contains("bad shelf id"));
    }
}
