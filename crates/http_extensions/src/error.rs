// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::time::Duration;

use http::StatusCode;
use http::header::{InvalidHeaderValue, MaxSizeReached};
use http::method::InvalidMethod;
use http::status::InvalidStatusCode;
use http::uri::{InvalidUri, InvalidUriParts};
use recoverable::{Recovery, RecoveryInfo};
use thread_aware::ThreadAware;
use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};

use crate::HttpErrorLabel;
use crate::HttpRequest;
use crate::http_utils::SyncHolder;

/// A convenient type alias for results in this crate.
pub type Result<T> = std::result::Result<T, HttpError>;

/// A unified HTTP error type.
///
/// Combines various HTTP-related errors into a single type with useful features:
///
/// - Captures backtraces automatically
/// - Tells you if an error is temporary (transient) or permanent
/// - Works with `http` crate errors out of the box
/// - Carries an [`HttpErrorLabel`] for metrics and logging (see its docs for
///   cardinality requirements)
///
/// # Examples
///
/// ```
/// # use http_extensions::HttpError;
/// # use recoverable::{Recovery, RecoveryKind};
///
/// fn check_error(error: HttpError) {
///     // See if we can retry
///     if error.recovery().kind() == RecoveryKind::Retry {
///         println!("temporary error, let's retry");
///     }
/// }
/// # check_error(HttpError::unavailable("test"));
/// ```
///
/// # Error Interoperability
///
/// Works with many error types through `From` implementations, so you can use
/// the `?` operator with them. Also tells you if errors can be recovered from.
///
/// ## Standard Library Errors
///
/// - [`std::io::Error`] - Auto-classified as retry, unavailable, or never based on error kind
/// - [`std::convert::Infallible`] - Handled for pattern matching completeness
///
/// ## Works with `http` crate
///
/// Converts from these error types automatically:
///
/// - `http::Error` - General HTTP errors
/// - `http::uri::InvalidUri` and `http::uri::InvalidUriParts` - Bad URIs
/// - `http::header::InvalidHeaderValue` - Invalid headers
/// - `http::method::InvalidMethod` - Invalid HTTP methods
/// - `http::status::InvalidStatusCode` - Invalid status codes
/// - `http::header::MaxSizeReached` - Headers too large
///
/// ```
/// # use http_extensions::HttpError;
///
/// let uri_error = "invalid uri".parse::<http::Uri>().unwrap_err();
/// let error = HttpError::from(uri_error);
///
/// assert!(error.to_string().starts_with("invalid uri character"));
/// ```
///
/// ## Works with `templated_uri` crate
///
/// - `templated_uri::ValidationError` - Invalid URI template parameters
///
/// ## Custom Errors
///
/// Custom errors can be wrapped using [`HttpError::other()`]:
///
/// ```
/// # use http_extensions::{HttpError};
/// # use recoverable::RecoveryInfo;
/// # use std::fmt;
/// # #[derive(Debug)]
/// # struct CustomError;
/// # impl std::error::Error for CustomError {}
/// # impl fmt::Display for CustomError {
/// #    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
/// #        write!(f, "custom error")
/// #    }
/// # }
///
/// let custom_error = CustomError;
/// let http_error = HttpError::other(custom_error, RecoveryInfo::never(), "custom");
/// ```
#[ohno::error]
#[from(
    http::Error(label: HttpErrorLabel::from("http_error"), recovery: RecoveryInfo::never()),
    InvalidUriParts(label: HttpErrorLabel::from("invalid_uri_parts"), recovery: RecoveryInfo::never()),
    InvalidUri(label: HttpErrorLabel::from("invalid_uri"), recovery: RecoveryInfo::never()),
    InvalidHeaderValue(label: HttpErrorLabel::from("invalid_header_value"), recovery: RecoveryInfo::never()),
    InvalidMethod(label: HttpErrorLabel::from("invalid_method"), recovery: RecoveryInfo::never()),
    InvalidStatusCode(label: HttpErrorLabel::from("invalid_status_code"), recovery: RecoveryInfo::never()),
    MaxSizeReached(label: HttpErrorLabel::from("max_size_reached"), recovery: RecoveryInfo::never()),
    std::io::Error(label: HttpErrorLabel::from(error.kind()), recovery: RecoveryInfo::from(error.kind())),
    templated_uri::ValidationError(label: HttpErrorLabel::from("invalid_uri"), recovery: RecoveryInfo::never())
)]
pub struct HttpError {
    label: HttpErrorLabel,
    recovery: RecoveryInfo,
    // NOTE: Boxed to keep the size of HttpError small and wrapped
    // in SyncHolder to make HttpError Sync even if HttpRequest is not Sync.
    request: Option<SyncHolder<Box<HttpRequest>>>,
}

impl ThreadAware for HttpError {
    fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
        // move as is
        self
    }
}

impl HttpError {
    /// Wraps any error type into an [`HttpError`] with the given `recovery`
    /// strategy and a `label` for metrics and logging.
    ///
    /// The `label` accepts anything that implements `Into<HttpErrorLabel>`.
    /// See [`HttpErrorLabel`] docs for cardinality requirements.
    pub fn other(
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
        recovery: RecoveryInfo,
        label: impl Into<HttpErrorLabel>,
    ) -> Self {
        Self::caused_by(label, recovery, None, error)
    }

    /// Wraps an error that implements [`Recovery`] into an [`HttpError`],
    /// extracting recovery information automatically via [`Recovery::recovery()`].
    ///
    /// The `label` accepts anything that implements `Into<HttpErrorLabel>`.
    /// See [`HttpErrorLabel`] docs for cardinality requirements.
    pub fn other_with_recovery<E>(error: E, label: impl Into<HttpErrorLabel>) -> Self
    where
        E: std::error::Error + Send + Sync + Recovery + 'static,
    {
        let recovery = error.recovery();

        Self::other(error, recovery, label)
    }

    /// Creates an error from an unsuccessful HTTP status `code` with the given
    /// `recovery` strategy.
    #[must_use]
    pub fn invalid_status_code(code: StatusCode, recovery: RecoveryInfo) -> Self {
        Self::other(
            format!("the response was not successful, status code: {}", code.as_u16()),
            recovery,
            "invalid_status_code",
        )
    }

    /// Creates a validation error.
    ///
    /// This is a convenience method to create a validation error with a standard message format.
    /// The error is classified as non-retryable.
    #[must_use]
    pub fn validation(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::other(msg.into(), RecoveryInfo::never(), "validation")
    }

    /// Creates an error that indicates a service is currently unavailable.
    ///
    /// This indicates that the service is currently down, unreachable, or
    /// experiencing an increased rate of failures.
    ///
    /// # Examples
    ///
    /// Reject the execution and attach the request for possible retry later. A typical case for
    /// this is an open circuit breaker that rejects executions without consuming the request.
    ///
    /// ```
    /// # use http_extensions::{HttpError, HttpRequest, HttpRequestBuilder};
    /// # let http_request = HttpRequestBuilder::new_fake()
    /// #     .get("https://example.com")
    /// #     .build()
    /// #     .unwrap();
    /// // attach the request
    /// let mut error = HttpError::unavailable("service is down").with_request(http_request);
    /// // later you can try to extract the request
    /// if let Some(request) = error.take_request() {
    ///    // execute the retry
    ///    execute_retry(request);
    /// }
    /// # fn execute_retry(http_request: HttpRequest) {}
    /// ```
    #[must_use]
    pub fn unavailable(msg: impl Into<Cow<'static, str>>) -> Self {
        Self::other(msg.into(), RecoveryInfo::unavailable(), "unavailable")
    }

    /// Creates a timeout error with the specified duration.
    ///
    /// This is a convenience method to create a timeout error with a standard message format.
    /// The error is classified as retryable.
    #[must_use]
    pub fn timeout(duration: Duration) -> Self {
        Self::other(
            format!(
                "request timed out while receiving the response, timeout: {}ms",
                duration.as_millis()
            ),
            RecoveryInfo::retry(),
            "timeout",
        )
    }

    /// Creates a timeout error for body data retrieval.
    ///
    /// Used when streaming body data is not fully received within the configured timeout.
    /// The error is classified as retryable.
    #[must_use]
    pub(crate) fn timeout_for_body(duration: Duration) -> Self {
        Self::other(
            format!("body data was not fully received, timeout: {}ms", duration.as_millis()),
            RecoveryInfo::retry(),
            "timeout",
        )
    }

    /// Attaches HTTP request to this error.
    ///
    /// Useful for rejected requests that you may want to retry later.
    #[must_use]
    pub fn with_request(mut self, request: HttpRequest) -> Self {
        self.request = Some(SyncHolder::new(Box::new(request)));
        self
    }

    /// Returns the [`HttpErrorLabel`] attached to this error.
    ///
    /// Labels are low-cardinality identifiers useful for metrics and logging.
    #[must_use]
    pub fn label(&self) -> &HttpErrorLabel {
        &self.label
    }

    /// Extracts the HTTP request from this error, if any.
    ///
    /// The request can be extracted only once. Further calls return `None`.
    /// The request can be attached to the error by using [`HttpError::with_request`]
    /// method.
    #[must_use]
    pub fn take_request(&mut self) -> Option<HttpRequest> {
        self.request.take().map(|holder| *holder.into_inner())
    }
}

impl Recovery for HttpError {
    fn recovery(&self) -> RecoveryInfo {
        self.recovery.clone()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::{Debug, Display};

    use ohno::ErrorExt;
    use recoverable::RecoveryKind;
    use thread_aware::affinity::pinned_affinities;

    use super::*;
    use crate::HttpRequestBuilder;

    static_assertions::assert_impl_all!(HttpError: std::error::Error, Send, Sync, Display, Debug, ThreadAware);

    #[test]
    fn assert_size_small() {
        // Keep the size of HttpError small to avoid excessive stack usage.
        assert_eq!(size_of::<HttpError>(), 64);
    }

    #[test]
    fn validation_ok() {
        let error = HttpError::validation("my-validation");

        assert_eq!(error.message(), "my-validation");
        assert_eq!(error.label(), "validation");
        assert_eq!(error.recovery(), RecoveryInfo::never());
    }

    #[test]
    fn invalid_status_code_ok() {
        let error = HttpError::invalid_status_code(StatusCode::NOT_FOUND, RecoveryInfo::unknown());

        assert_eq!(error.message(), "the response was not successful, status code: 404");
        assert_eq!(error.label(), "invalid_status_code");
        assert_eq!(error.recovery(), RecoveryInfo::unknown());
    }

    #[test]
    fn other_method_wraps_custom_errors() {
        let io_error = std::io::Error::other("custom error");
        let error = HttpError::other(io_error, RecoveryInfo::retry(), "custom");

        assert_eq!(error.message(), "custom error");
        assert_eq!(error.label(), "custom");
        assert_eq!(error.recovery(), RecoveryInfo::retry());
    }

    #[test]
    fn http_constructor() {
        let invalid_method = http::Method::from_bytes(b"INVALID METHOD").unwrap_err();
        let error = HttpError::from(invalid_method);
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_eq!(error.label(), "invalid_method");
    }

    #[test]
    fn from_io() {
        let error = HttpError::from(std::io::Error::other("test"));
        assert_eq!(error.message(), "test");
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_eq!(error.label(), "other");

        let error = HttpError::from(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "some message"));
        assert_eq!(error.recovery(), RecoveryInfo::retry());
        assert_eq!(error.label(), "broken_pipe");
    }

    #[test]
    fn from_uri_errors() {
        let uri_error = "invalid uri with spaces".parse::<http::Uri>().unwrap_err();
        let error = HttpError::from(uri_error);
        assert_eq!(error.recovery(), RecoveryInfo::never());
        assert_eq!(error.label(), "invalid_uri");
    }

    #[test]
    fn assert_from() {
        static_assertions::assert_impl_all!(HttpError: From<http::Error>);
        static_assertions::assert_impl_all!(HttpError: From<InvalidUri>);
        static_assertions::assert_impl_all!(HttpError: From<InvalidUriParts>);
        static_assertions::assert_impl_all!(HttpError: From<InvalidHeaderValue>);
        static_assertions::assert_impl_all!(HttpError: From<InvalidMethod>);
        static_assertions::assert_impl_all!(HttpError: From<InvalidStatusCode>);
        static_assertions::assert_impl_all!(HttpError: From<MaxSizeReached>);
        static_assertions::assert_impl_all!(HttpError: From<templated_uri::ValidationError>);
        static_assertions::assert_impl_all!(HttpError: From<std::io::Error>);
    }

    #[test]
    fn assert_from_infallible() {
        static_assertions::assert_impl_all!(HttpError: From<std::convert::Infallible>);
    }

    #[test]
    fn timeout_error() {
        let duration = Duration::from_millis(1500);
        let timeout_error = HttpError::timeout(duration);

        assert_eq!(timeout_error.recovery(), RecoveryInfo::retry());
        assert_eq!(
            timeout_error.message(),
            "request timed out while receiving the response, timeout: 1500ms"
        );
        assert_eq!(timeout_error.label(), "timeout");
    }

    #[test]
    fn timeout_for_body_error() {
        let duration = Duration::from_millis(2500);
        let error = HttpError::timeout_for_body(duration);

        assert_eq!(error.recovery(), RecoveryInfo::retry());
        assert_eq!(error.message(), "body data was not fully received, timeout: 2500ms");
        assert_eq!(error.label(), "timeout");
    }

    #[test]
    fn unavailable_error() {
        let unavailable_error = HttpError::unavailable("service is down");

        assert_eq!(unavailable_error.recovery(), RecoveryInfo::unavailable());
        assert_eq!(unavailable_error.message(), "service is down");
        assert_eq!(unavailable_error.label(), "unavailable");
    }

    #[test]
    fn other_with_recovery() {
        let existing_error = HttpError::validation("base error");
        let error = HttpError::other_with_recovery(existing_error, "permission");

        assert!(error.message().contains("base error"));
        assert_eq!(error.label(), "permission");
        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
    }

    #[test]
    fn rejected_request_ok() {
        let request = HttpRequestBuilder::new_fake().uri("https://dummy").build().unwrap();

        let mut error = HttpError::validation("rejection").with_request(request);

        assert_eq!(error.take_request().unwrap().uri().to_string(), "https://dummy/");

        // Later calls should return None
        assert!(error.take_request().is_none());
    }

    #[test]
    fn relocated_preserves_error() {
        let affinity = pinned_affinities(&[1])[0];
        let error = HttpError::validation("relocated test");

        let relocated = error.relocated(MemoryAffinity::Unknown, affinity);

        assert_eq!(relocated.message(), "relocated test");
        assert_eq!(relocated.label(), "validation");
    }

    #[test]
    fn other_with_owned_label() {
        let owned_label = String::from("dynamic_label");
        let error = HttpError::other(std::io::Error::other("some error"), RecoveryInfo::never(), owned_label);
        assert_eq!(error.label(), "dynamic_label");
    }
}
