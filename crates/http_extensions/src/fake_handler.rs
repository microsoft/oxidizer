// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use http::{Response, StatusCode};
use layered::Service;
use thread_aware::ThreadAware;
use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};

use crate::constants::ERR_POISONED_LOCK;
use crate::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, Result};

type PinnedFuture = Pin<Box<dyn Future<Output = Result<HttpResponse>> + Send>>;

/// Simulates HTTP responses for testing without making actual network requests.
///
/// The [`FakeHandler`] lets you easily mock HTTP client behavior by providing predefined
/// responses or custom response logic. Available with the `test-util` feature enabled.
///
/// # Examples
///
/// Return a fixed status code:
///
/// ```
/// use http::StatusCode;
/// use http_extensions::FakeHandler;
///
/// // All requests to this client will return 404 Not Found
/// let handler = FakeHandler::from(StatusCode::NOT_FOUND);
/// ```
///
/// Return a sequence of responses:
///
/// ```
/// use http::StatusCode;
/// use http_extensions::FakeHandler;
///
/// let handler = FakeHandler::from(vec![
///     StatusCode::OK,
///     StatusCode::BAD_REQUEST,
///     StatusCode::INTERNAL_SERVER_ERROR,
/// ]);
///
/// // First request → 200 OK
/// // Second request → 400 Bad Request
/// // Third request → 500 Internal Server Error
/// // Fourth request → Error (all responses consumed)
/// ```
///
/// Create dynamic responses based on the request:
///
/// ```
/// use http::StatusCode;
/// use http_extensions::{FakeHandler, HttpResponseBuilder};
///
/// let handler = FakeHandler::from_sync_handler(|request| {
///     if request.uri().path() == "/api/users" {
///         HttpResponseBuilder::new_fake()
///             .status(StatusCode::OK)
///             .text(r#"{"users": []}"#)
///             .build()
///     } else {
///         HttpResponseBuilder::new_fake()
///             .status(StatusCode::NOT_FOUND)
///             .text("Resource not found")
///             .build()
///     }
/// });
/// ```
///
/// # Working with [`HttpResponseBuilder`][crate::HttpResponseBuilder]
///
/// Use [`HttpResponseBuilder::new_fake`][crate::HttpResponseBuilder::new_fake] to generate tailored test responses
/// with a fluent API. This is especially useful with [`FakeHandler::from_sync_handler`]
/// to create responses that react to request data:
#[derive(Clone, Debug)]
pub struct FakeHandler {
    inner: Arc<Inner>,
    http_body_builder: HttpBodyBuilder,
}

impl ThreadAware for FakeHandler {
    fn relocated(self, _source: MemoryAffinity, _destination: PinnedAffinity) -> Self {
        // No thread awareness needed for fake handler, we want the same behavior
        // event after relocation.
        self
    }
}

impl AsRef<HttpBodyBuilder> for FakeHandler {
    fn as_ref(&self) -> &HttpBodyBuilder {
        &self.http_body_builder
    }
}

impl FakeHandler {
    fn new(inner: Inner) -> Self {
        Self {
            inner: Arc::new(inner),
            http_body_builder: HttpBodyBuilder::new_fake(),
        }
    }

    /// Creates a handler from a synchronous request handler function.
    ///
    /// Takes a function that processes requests synchronously and wraps it into
    /// an async handler internally.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::{FakeHandler, HttpResponseBuilder};
    ///
    /// let handler = FakeHandler::from_sync_handler(|request| {
    ///     HttpResponseBuilder::new_fake()
    ///         .status(StatusCode::OK)
    ///         .text("Hello World")
    ///         .build()
    /// });
    /// ```
    pub fn from_sync_handler<H>(handler: H) -> Self
    where
        H: Fn(HttpRequest) -> Result<HttpResponse> + 'static + Send + Sync,
    {
        let handler = Arc::new(handler);
        Self::from_async_handler(move |req| {
            let cloned = Arc::clone(&handler);
            async move { cloned(req) }
        })
    }

    /// Creates a handler that always returns HTTP error.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_extensions::{FakeHandler, HttpError, HttpRequest};
    ///
    /// let handler = FakeHandler::from_http_error(|_request: HttpRequest| {
    ///    HttpError::validation("simulated error")
    /// });
    pub fn from_http_error(error: impl Fn(HttpRequest) -> HttpError + Send + Sync + 'static) -> Self {
        Self::from_sync_handler(move |req| Err(error(req)))
    }

    /// Creates a handler from an asynchronous function.
    ///
    /// Useful for complex async scenarios like simulating network delays or
    /// other asynchronous behavior in your tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::{FakeHandler, HttpResponseBuilder};
    ///
    /// let handler = FakeHandler::from_async_handler(|request| async move {
    ///     // Simulate network delay
    ///     tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    ///
    ///     HttpResponseBuilder::new_fake()
    ///         .status(StatusCode::OK)
    ///         .text("Response after delay")
    ///         .build()
    /// });
    /// ```
    pub fn from_async_handler<H, F>(handler: H) -> Self
    where
        H: Fn(HttpRequest) -> F + 'static + Send + Sync,
        F: Future<Output = Result<HttpResponse>> + Send + 'static,
    {
        Self::new(Inner::Custom(Box::new(move |req| Box::pin(handler(req)))))
    }

    /// Creates a handler that returns a sequence of status codes.
    ///
    /// Returns empty-body responses with the given status codes in order.
    /// When the sequence is exhausted, further requests will error.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::FakeHandler;
    ///
    /// let handler = FakeHandler::from_status_codes([
    ///     StatusCode::OK,
    ///     StatusCode::BAD_REQUEST,
    ///     StatusCode::INTERNAL_SERVER_ERROR,
    /// ]);
    /// ```
    ///
    /// # Errors
    ///
    /// After all responses are consumed, further requests will return a
    /// validation error.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the panic never happens as the body creation is guaranteed to be valid"
    )]
    pub fn from_status_codes<T>(codes: T) -> Self
    where
        T: IntoIterator<Item = StatusCode>,
    {
        Self::from_responses(codes.into_iter().map(|code| {
            Response::builder()
                .status(code)
                .body(HttpBodyBuilder::new_fake().empty())
                .expect("we know that the response is valid")
        }))
    }

    /// Creates a handler that returns predefined HTTP responses in sequence.
    ///
    /// Takes full [`HttpResponse`] objects and returns them in order.
    /// When the sequence is exhausted, further requests return an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use http_extensions::{FakeHandler, HttpResponseBuilder};
    ///
    /// let responses = vec![
    ///     HttpResponseBuilder::new_fake()
    ///         .status(StatusCode::OK)
    ///         .text("Success response")
    ///         .build()
    ///         .unwrap(),
    ///     HttpResponseBuilder::new_fake()
    ///         .status(StatusCode::NOT_FOUND)
    ///         .text("Not found")
    ///         .build()
    ///         .unwrap(),
    /// ];
    ///
    /// let handler = FakeHandler::from_responses(responses);
    /// ```
    ///
    /// # Errors
    ///
    /// After all responses are consumed, requests will fail with
    /// "all responses used by fake handler are already consumed".
    pub fn from_responses<T>(responses: T) -> Self
    where
        T: IntoIterator<Item = HttpResponse>,
    {
        Self::new(Inner::Multiple(Mutex::new(responses.into_iter().collect())))
    }

    /// Creates a handler that never completes requests.
    ///
    /// Useful for testing timeout handling in your code.
    pub fn never_completes() -> Self {
        Self::new(Inner::NeverCompletes)
    }
}

/// Create a [`FakeHandler`] from a vector of status codes.
impl From<Vec<StatusCode>> for FakeHandler {
    fn from(value: Vec<StatusCode>) -> Self {
        Self::from_status_codes(value)
    }
}

/// Create a [`FakeHandler`] from a vector of HTTP responses.
impl From<Vec<HttpResponse>> for FakeHandler {
    fn from(value: Vec<HttpResponse>) -> Self {
        Self::from_responses(value)
    }
}

/// Create a [`FakeHandler`] from a single status code.
impl From<StatusCode> for FakeHandler {
    fn from(value: StatusCode) -> Self {
        Self::new(Inner::StatusCode(value, HttpBodyBuilder::new_fake()))
    }
}

/// Create a [`FakeHandler`] from a single HTTP response.
///
/// The response body must be buffered to be reused for multiple requests. If the body
/// is not buffered, an error will be returned when the handler tries to reuse it.
impl From<HttpResponse> for FakeHandler {
    fn from(value: HttpResponse) -> Self {
        let (parts, body) = value.into_parts();
        let body = MaybeUnbufferedBody(Mutex::new(body));

        Self::from_sync_handler(move |_| {
            let data = body.get_data()?;
            Ok(Response::from_parts(parts.clone(), data))
        })
    }
}

impl Service<HttpRequest> for FakeHandler {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        Box::pin(self.inner.send_request(input))
    }
}

/// Creates a default [`FakeHandler`] that returns 200 OK responses.
impl Default for FakeHandler {
    fn default() -> Self {
        Self::from(StatusCode::OK)
    }
}

enum Inner {
    StatusCode(StatusCode, HttpBodyBuilder),
    Multiple(Mutex<VecDeque<HttpResponse>>),
    Custom(Box<dyn Fn(HttpRequest) -> PinnedFuture + Send + Sync + 'static>),
    NeverCompletes,
}

impl Inner {
    async fn send_request(&self, request: HttpRequest) -> Result<HttpResponse> {
        match self {
            Self::Multiple(responses) => responses
                .lock()
                .expect("mutex poisoned")
                .pop_front()
                .ok_or_else(|| HttpError::validation("all responses used by fake handler are already consumed")),
            Self::Custom(handler) => handler(request).await,
            Self::NeverCompletes => std::future::pending().await,
            Self::StatusCode(code, creator) => Ok(Response::builder().status(code).body(creator.empty()).expect("works")),
        }
    }
}

impl Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StatusCode(code, _) => write!(f, "StatusCode({code})"),
            Self::Multiple(_) => write!(f, "Multiple"),
            Self::Custom(_) => write!(f, "Custom"),
            Self::NeverCompletes => write!(f, "NeverCompletes"),
        }
    }
}

struct MaybeUnbufferedBody(Mutex<HttpBody>);

impl MaybeUnbufferedBody {
    fn get_data(&self) -> crate::Result<HttpBody> {
        let body = self
            .0
            .lock()
            .expect(ERR_POISONED_LOCK)
            .try_clone()
            .ok_or_else(|| HttpError::validation("the HTTP response body must be buffered to be reused in FakeHandler"))?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::vec;

    use futures::executor::block_on;
    use http_body_util::Empty;
    use ohno::{ErrorExt, assert_error_message};
    use thread_aware::affinity::pinned_affinities;
    use tick::{ClockControl, FutureExt};

    use super::*;
    use crate::HttpResponseBuilder;
    use crate::request_handler::RequestHandlerExt;

    #[test]
    fn from_status_code_ok() -> std::result::Result<(), ohno::AppError> {
        let handler = FakeHandler::from(StatusCode::NOT_IMPLEMENTED);

        for _ in 0..3 {
            // Providing status code works indefinitely
            assert_eq!(get_response(&handler)?.status(), StatusCode::NOT_IMPLEMENTED);
        }

        Ok(())
    }

    #[test]
    fn from_status_codes_ok() -> std::result::Result<(), ohno::AppError> {
        let handler = FakeHandler::from(vec![StatusCode::NOT_IMPLEMENTED, StatusCode::BAD_REQUEST]);

        assert_eq!(get_response(&handler)?.status(), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(get_response(&handler)?.status(), StatusCode::BAD_REQUEST);
        assert!(
            get_response(&handler)
                .unwrap_err()
                .to_string()
                .starts_with("all responses used by fake handler are already consumed")
        );

        Ok(())
    }

    #[test]
    fn from_responses_ok() -> std::result::Result<(), ohno::AppError> {
        let handler = FakeHandler::from(vec![
            HttpResponseBuilder::new_fake().status(StatusCode::OK).text("Response 1").build()?,
            HttpResponseBuilder::new_fake().status(StatusCode::OK).text("Response 2").build()?,
        ]);

        assert_eq!(get_response_text(&handler)?, "Response 1");
        assert_eq!(get_response_text(&handler)?, "Response 2");
        assert!(
            get_response(&handler)
                .unwrap_err()
                .to_string()
                .starts_with("all responses used by fake handler are already consumed")
        );

        Ok(())
    }

    #[test]
    fn from_sync_handler_ok() -> std::result::Result<(), ohno::AppError> {
        let handler =
            FakeHandler::from_sync_handler(|_request| HttpResponseBuilder::new_fake().status(StatusCode::OK).text("Sync response").build());

        assert_eq!(get_response_text(&handler)?, "Sync response");

        Ok(())
    }

    #[test]
    fn from_async_handler_ok() -> std::result::Result<(), ohno::AppError> {
        let handler = FakeHandler::from_async_handler(|_request| async move {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .text("Async response")
                .build()
        });

        assert_eq!(get_response_text(&handler)?, "Async response");

        Ok(())
    }

    #[test]
    fn never_completes_handler() {
        let handler = FakeHandler::never_completes();
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let error = block_on(
            handler
                .request_builder()
                .get("https://dummy")
                .fetch()
                .timeout(&clock, Duration::from_secs(1)),
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "future timed out");
    }

    #[test]
    fn from_single_response_ok() -> std::result::Result<(), ohno::AppError> {
        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::CREATED)
            .text("Single response")
            .build()?;
        let handler = FakeHandler::from(response);

        for _ in 0..3 {
            // Single response handlers should work indefinitely
            assert_eq!(get_response_text(&handler)?, "Single response");
        }

        Ok(())
    }

    #[test]
    fn assert_clone_implemented() {
        static_assertions::assert_impl_all!(FakeHandler: Clone);
    }

    #[test]
    fn empty_responses_error() {
        let empty_codes: Vec<StatusCode> = vec![];
        let handler = FakeHandler::from(empty_codes);

        assert_eq!(
            get_response(&handler).unwrap_err().message(),
            "all responses used by fake handler are already consumed"
        );
    }

    #[test]
    fn async_handler_returns_error() {
        let handler =
            FakeHandler::from_async_handler(|_request| async { Err(HttpError::validation("this is a test error from async handler")) });

        // Next call should produce a specific error message
        let error = get_response(&handler).unwrap_err();
        assert_eq!(error.message(), "this is a test error from async handler");
    }

    #[test]
    fn default_returns_ok() -> std::result::Result<(), ohno::AppError> {
        let handler = FakeHandler::default();
        assert_eq!(get_response(&handler)?.status(), StatusCode::OK);
        Ok(())
    }

    #[test]
    fn from_response_ok() {
        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .text("Test body")
            .build()
            .unwrap();

        let handler = FakeHandler::from(response);

        assert_eq!(get_response_text(&handler).unwrap(), "Test body");
        // Subsequent calls should also succeed
        assert_eq!(get_response_text(&handler).unwrap(), "Test body");
    }

    #[test]
    fn from_response_unbuffered_error() {
        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::OK)
            .external(Empty::new())
            .build()
            .unwrap();

        let handler = FakeHandler::from(response);

        assert_error_message!(
            get_response_text(&handler).unwrap_err().message(),
            "the HTTP response body must be buffered to be reused in FakeHandler"
        );
    }

    #[test]
    fn from_http_error() {
        let handler = FakeHandler::from_http_error(|_request| HttpError::validation("simulated error"));

        let error = get_response(&handler).unwrap_err();
        assert_eq!(error.message(), "simulated error");
    }

    #[test]
    fn relocated_preserves_behavior() {
        let affinity = pinned_affinities(&[2])[0];
        let handler = FakeHandler::from(StatusCode::ACCEPTED);

        let relocated = handler.relocated(MemoryAffinity::Unknown, affinity);

        let status = get_response(&relocated).unwrap().status();
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    fn get_response(client: &FakeHandler) -> Result<HttpResponse> {
        block_on(client.request_builder().get("https://dummy").fetch())
    }

    fn get_response_text(client: &FakeHandler) -> Result<String> {
        Ok(block_on(client.request_builder().get("https://dummy").fetch_text())?.into_body())
    }
}
