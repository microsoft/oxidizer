// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Status`] gRPC status message type.

use core::fmt;
use std::error::Error;

use serde_json::Value;

use crate::code::Code;

/// A gRPC status: a [`Code`], a human-readable message, and optional
/// `google.rpc.Status`-style details.
///
/// Generated service-trait methods return `Result<Response, Status>`; a
/// generated transcoder maps the [`Code`] to an HTTP status via
/// [`Code::to_http_status`](crate::handling::Code::to_http_status) and renders the status as a
/// JSON body of the form `{"code": <i32>, "message": <string>, "details": [ … ]}`
/// (the `details` array is omitted when empty).
///
/// # Examples
///
/// ```
/// use rest_over_grpc::handling::{Code, Status};
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
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::derive_partial_eq_without_eq,
    reason = "details hold serde_json::Value, which is not Eq (nor Hash)"
)]
pub struct Status {
    code: Code,
    message: String,
    details: Vec<Value>,
}

impl Status {
    /// Creates a status with the given `code` and `message` and no details.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::{Code, Status};
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
            details: Vec::new(),
        }
    }

    /// Creates a [`Code::InvalidArgument`] status.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::{Code, Status};
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
    /// use rest_over_grpc::handling::{Code, Status};
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
    /// use rest_over_grpc::handling::{Code, Status};
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
    /// use rest_over_grpc::handling::{Code, Status};
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
    /// use rest_over_grpc::handling::Status;
    ///
    /// let status = Status::invalid_argument("bad request");
    /// assert_eq!(status.message(), "bad request");
    /// ```
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Attaches a single detail value, returning the updated status.
    ///
    /// Details mirror `google.rpc.Status.details`: arbitrary structured values
    /// (typically a serialized `google.protobuf.Any`, e.g. an `ErrorInfo` or
    /// `BadRequest`) that the transcoder renders into the JSON error body's
    /// `details` array. Call repeatedly to attach more.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::{Code, Status};
    /// use serde_json::json;
    ///
    /// let status = Status::invalid_argument("shelf id must be numeric").with_detail(json!({
    ///     "@type": "type.googleapis.com/google.rpc.BadRequest",
    ///     "fieldViolations": [{ "field": "shelf", "description": "must be numeric" }],
    /// }));
    /// assert_eq!(status.details().len(), 1);
    /// ```
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<Value>) -> Self {
        self.details.push(detail.into());
        self
    }

    /// Attaches a sequence of detail values, returning the updated status.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::{Code, Status};
    /// use serde_json::json;
    ///
    /// let status = Status::new(Code::FailedPrecondition, "quota exceeded")
    ///     .with_details([json!({ "reason": "QUOTA" }), json!({ "retryAfter": "30s" })]);
    /// assert_eq!(status.details().len(), 2);
    /// ```
    #[must_use]
    pub fn with_details(mut self, details: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        self.details.extend(details.into_iter().map(Into::into));
        self
    }

    /// The status details (the `google.rpc.Status.details` array), empty when
    /// none were attached.
    ///
    /// # Examples
    ///
    /// ```
    /// use rest_over_grpc::handling::Status;
    ///
    /// assert!(Status::not_found("gone").details().is_empty());
    /// ```
    #[must_use]
    pub fn details(&self) -> &[Value] {
        &self.details
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for Status {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_detail_appends_a_single_detail_value() {
        let status = Status::invalid_argument("bad").with_detail(serde_json::json!({ "field": "shelf" }));
        assert_eq!(status.details().len(), 1);
        assert_eq!(status.details()[0]["field"], "shelf");
    }

    #[test]
    fn display_uses_canonical_code_name() {
        assert_eq!(Status::not_found("gone").to_string(), "NOT_FOUND: gone");
    }
}
