// Copyright (c) Microsoft Corporation.

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
/// - Provides access to status codes
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
/// ```
///
/// # Error Interoperability
///
/// Works with many error types through `From` implementations, so you can use
/// the `?` operator with them. Also tells you if errors can be recovered from.
///
/// ## Standard Library Errors
///
/// - [`std::io::Error`] - Auto-classified as temporary or permanent
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
/// ## Custom Errors
///
/// Got a custom error? No problem! Use `Error::other()` to wrap it.
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
///
/// [http]: https://docs.rs/http
#[ohno::error]
#[from(
    http::Error(label: "http_error", recovery: RecoveryInfo::never()),
    InvalidUriParts(label: "invalid_uri_parts", recovery: RecoveryInfo::never()),
    InvalidUri(label: "invalid_uri", recovery: RecoveryInfo::never()),
    InvalidHeaderValue(label: "invalid_header_value", recovery: RecoveryInfo::never()),
    InvalidMethod(label: "invalid_method", recovery: RecoveryInfo::never()),
    InvalidStatusCode(label: "invalid_status_code", recovery: RecoveryInfo::never()),
    MaxSizeReached(label: "max_size_reached", recovery: RecoveryInfo::never()),
    std::io::Error(label: "io", recovery: crate::resilience::detect_io_recovery(error.kind())),
    templated_uri::ValidationError(label: "invalid_uri", recovery: RecoveryInfo::never())
)]
pub struct HttpError {
    label: &'static str,
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
    /// Creates a new Error from any error type.
    ///
    /// A flexible way to wrap your own errors in our Error type.
    ///
    /// # Parameters
    ///
    /// - `error`: Any error that can become a boxed error trait object
    /// - `recovery`: Recovery information for this error
    /// - `label`: A low-cardinality label for this error (for metrics/logging)
    pub fn other(
        error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
        recovery: RecoveryInfo,
        label: &'static str,
    ) -> Self {
        Self::caused_by(label, recovery, None, error)
    }

    /// Creates a new error from any error type that implements `Recovery`.
    ///
    /// # Parameters
    ///
    /// - `error`: Any error that can become a boxed error trait object
    /// - `label`: A low-cardinality label for this error (for metrics/logging)
    pub fn other_with_recovery<E>(error: E, label: &'static str) -> Self
    where
        E: std::error::Error + Send + Sync + Recovery + 'static,
    {
        let recovery = error.recovery();

        Self::other(error, recovery, label)
    }

    /// Creates a new Error with a specific HTTP status code.
    ///
    /// Perfect for when you got a bad HTTP status code as a response.
    ///
    /// # Parameters
    ///
    /// - `code`: The HTTP status code
    /// - `recovery`: Recovery information for this error
    #[must_use]
    pub fn invalid_status_code(code: StatusCode, recovery: RecoveryInfo) -> Self {
        Self::other(
            format!(
                "the response was not successful, status code: {}",
                code.as_u16()
            ),
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

    /// Creates error that indicates a service is currently unavailable.
    ///
    /// This indicates that service is currently down, unreachable or
    /// experiences increased rate of failures.
    ///
    /// # Examples
    ///
    /// Reject the execution and attach the request for possible retry later. Typical case for this
    /// is opened circuit breaker, that rejects the executions without consuming the request.
    ///
    /// ```
    /// # use http_extensions::{HttpError, HttpRequest, HttpRequestBuilder};
    /// # fn example(http_request: HttpRequest) {
    /// // attach the request
    /// let mut error = HttpError::unavailable("service is down").with_request(http_request);
    /// // later you can try to extract the request
    /// if let Some(request) = error.take_request() {
    ///    // execute the retry
    ///    execute_retry(request);
    /// }
    /// # }
    /// # fn execute_retry(http_request: HttpRequest) {}
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

    /// Attaches HTTP request to this error.
    ///
    /// Useful for rejected requests that you may want to retry later.
    #[must_use]
    pub fn with_request(mut self, request: HttpRequest) -> Self {
        self.request = Some(SyncHolder::new(Box::new(request)));
        self
    }

    /// Low-cardinality label for this error.
    ///
    /// Useful for metrics and logging.
    #[must_use]
    pub fn label(&self) -> &'static str {
        self.label
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
mod tests {
    use std::fmt::{Debug, Display};

    use ohno::ErrorExt;
    use recoverable::RecoveryKind;

    use super::*;
    use crate::HttpRequestBuilder;

    static_assertions::assert_impl_all!(HttpError: std::error::Error, Send, Sync, Display, Debug, ThreadAware);

    #[test]
    fn assert_size_small() {
        // Keep the size of HttpError small to avoid excessive stack usage.
        assert_eq!(size_of::<HttpError>(), 56);
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

        assert_eq!(
            error.message(),
            "the response was not successful, status code: 404"
        );
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
        assert_eq!(error.label(), "io");
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
        let request = HttpRequestBuilder::new_fake()
            .uri("https://dummy")
            .build()
            .unwrap();

        let mut error = HttpError::validation("rejection").with_request(request);

        assert_eq!(
            error.take_request().unwrap().uri().to_string(),
            "https://dummy/"
        );

        // Later calls should return None
        assert!(error.take_request().is_none());
    }
}
