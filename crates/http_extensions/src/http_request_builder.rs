// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::future::ready;

use bytesbuf::BytesView;
use futures::Stream;
use futures::future::Either;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Response, Version};
use templated_uri::Uri;

use crate::http_utils::{CONTENT_TYPE_TEXT, try_content_length_header, try_header};
use crate::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, RequestHandler, Result};

/// A fluent builder for creating HTTP requests.
///
/// `HttpRequestBuilder` simplifies the process of building HTTP requests by providing a chainable API.
/// It handles setting headers, different body types, and offers convenient methods for common
/// request building patterns.
///
/// # Creating a Request Builder
///
/// There are two main ways to create an `HttpRequestBuilder`:
///
/// 1. **Without a request handler** - Use [`new`](Self::new) to create a builder that can only
///    build requests via [`build`](Self::build):
///
///    ```
///    # use http::Method;
///    # use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder};
///    # fn example(body_creator: &HttpBodyBuilder) -> Result<(), HttpError> {
///    let request_builder = HttpRequestBuilder::new(body_creator);
///    let request: HttpRequest = request_builder
///        .method(Method::POST)
///        .uri("https://example.com/api")
///        .text("Hello world")
///        .build()?;
///    # Ok(())
///    # }
///    ```
///
/// 2. **With a request handler** - Use [`with_request_handler`](Self::with_request_handler) to create
///    a builder that can send requests directly using fetch methods like [`fetch`](Self::fetch) or
///    [`fetch_text`](Self::fetch_text):
///
///    ```
///    # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponse, RequestHandler, HttpRequestBuilder};
///    # async fn example<R: RequestHandler + Clone>(
///    #     request_handler: &R,
///    #     body_creator: &HttpBodyBuilder
///    # ) -> Result<(), HttpError> {
///    let response: HttpResponse = HttpRequestBuilder::with_request_handler(request_handler, body_creator)
///        .get("https://example.com/api")
///        .fetch()
///        .await?;
///    # Ok(())
///    # }
///    ```
#[derive(Debug)]
#[must_use]
pub struct HttpRequestBuilder<'a, R = ()> {
    body_builder: Cow<'a, HttpBodyBuilder>,
    builder: http::request::Builder,
    uri: Option<Result<Uri>>,
    body: Option<Result<HttpBody>>,
    content_type: Option<HeaderValue>,
    request_handler: &'a R,
}

impl HttpRequestBuilder<'static> {
    /// Creates a new request builder instance for testing.
    ///
    /// This method provides a convenient way to create a `HttpRequestBuilder` for tests
    /// without needing an existing body creator. The request builder is ready to be
    /// configured with headers, method, URI, and body.
    ///
    /// The `test-util` feature must be enabled to use this method.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::Method;
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder};
    /// # fn example() -> Result<(), HttpError> {
    /// let request = HttpRequestBuilder::new_fake()
    ///     .method(Method::GET)
    ///     .uri("https://example.com")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(any(feature = "test-util", test))]
    pub fn new_fake() -> Self {
        Self {
            body_builder: Cow::Owned(HttpBodyBuilder::new_fake()),
            builder: http::request::Builder::new(),
            uri: None,
            body: None,
            content_type: None,
            request_handler: &(),
        }
    }
}

impl<'a> HttpRequestBuilder<'a> {
    /// Creates a new request builder instance with the given body creator.
    pub fn new(creator: &'a HttpBodyBuilder) -> Self {
        Self {
            body_builder: Cow::Borrowed(creator),
            builder: http::request::Builder::new(),
            uri: None,
            body: None,
            content_type: None,
            request_handler: &(),
        }
    }
}

impl<'a, R> HttpRequestBuilder<'a, R> {
    /// Creates a new request builder instance with the given body creator and request handler.
    pub fn with_request_handler(request_handler: &'a R, body_builder: &'a HttpBodyBuilder) -> Self {
        Self {
            builder: http::request::Builder::new(),
            body_builder: Cow::Borrowed(body_builder),
            uri: None,
            body: None,
            content_type: None,
            request_handler,
        }
    }
}

impl<R> HttpRequestBuilder<'_, R> {
    /// Sets the HTTP method for the request.
    pub fn method(mut self, method: impl TryInto<http::Method, Error: Into<http::Error>>) -> Self {
        self.builder = self.builder.method(method);
        self
    }

    /// Sets the URI for the request.
    pub fn uri(mut self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri = Some(uri.try_into().map_err(Into::into));
        self
    }

    /// Sets a GET method and URI for the request.
    pub fn get(self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri(uri).method(Method::GET)
    }

    /// Creates a POST request to the specified URI.
    pub fn post(self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri(uri).method(Method::POST)
    }

    /// Creates a DELETE request to the specified URI.
    pub fn delete(self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri(uri).method(Method::DELETE)
    }

    /// Creates a PUT request to the specified URI.
    pub fn put(self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri(uri).method(Method::PUT)
    }

    /// Creates a PATCH request to the specified URI.
    pub fn patch(self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri(uri).method(Method::PATCH)
    }

    /// Creates a HEAD request to the specified URI.
    pub fn head(self, uri: impl TryInto<Uri, Error: Into<HttpError>>) -> Self {
        self.uri(uri).method(Method::HEAD)
    }

    /// Provides mutable access to the request headers.
    ///
    /// Use this when you need to manipulate headers directly.
    /// For simple header addition, prefer using the [`header`](Self::header) method.
    ///
    /// When the builder has errors, this method will return `None`.
    pub fn headers_mut(&mut self) -> Option<&mut HeaderMap<HeaderValue>> {
        self.builder.headers_mut()
    }

    /// Adds a header to the request.
    ///
    /// This method accepts any type that can be converted to a [`HeaderName`] and [`HeaderValue`].
    /// It returns `self` to enable method chaining.
    ///
    /// # Performance
    ///
    /// It's better to use pre-created `HeaderName` and `HeaderValue` instances to avoid
    /// parsing overhead. This applies for values that are fixed and used multiple times.
    pub fn header(
        mut self,
        key: impl TryInto<HeaderName, Error: Into<http::Error>>,
        value: impl TryInto<HeaderValue, Error: Into<http::Error>>,
    ) -> Self {
        self.builder = self.builder.header(key, value);
        self
    }

    /// Sets the HTTP protocol version for the request.
    pub fn version(mut self, version: Version) -> Self {
        self.builder = self.builder.version(version);
        self
    }

    /// Adds an extension to the request.
    ///
    /// Extensions are type-mapped data that can be attached to requests for use by
    /// middleware, handlers, or other parts of your application.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpRequestBuilder;
    /// #[derive(Clone)]
    /// struct RequestId(String);
    ///
    /// let request = HttpRequestBuilder::new_fake()
    ///     .get("https://example.com/api/users/123")
    ///     .extension(RequestId("req-456".to_string()))
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn extension<T>(mut self, extension: T) -> Self
    where
        T: Clone + Send + Sync + 'static,
    {
        self.builder = self.builder.extension(extension);
        self
    }

    /// Sets a plain text body for the request.
    ///
    /// Automatically sets the `Content-Type` header to `text/plain`.
    /// If the `Content-Type` header is already set, it will not override it.
    ///
    /// This method always encodes the provided string as UTF-8.
    pub fn text(mut self, data: impl AsRef<str>) -> Self {
        let body = self.body_builder.text(data);
        self.content_type = Some(CONTENT_TYPE_TEXT);
        self.body(body)
    }

    /// Sets a byte sequence as the request body.
    ///
    /// Use this when you need to send raw binary data.
    /// Unlike [`text`](Self::text), this doesn't set a `Content-Type` header.
    pub fn bytes(self, b: impl Into<BytesView>) -> Self {
        let body = self.body_builder.bytes(b);
        self.body(body)
    }

    /// Sets a JSON-serialized body for the request.
    ///
    /// Takes any type that implements `serde::Serialize` and converts it to JSON with the following rules:
    ///
    /// - The `Content-Type` header is set to `application/json` if not already set.
    /// - The data is always encoded as UTF-8.
    ///
    /// This method requires the `json` feature to be enabled.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    #[cfg(any(feature = "json", test))]
    pub fn json<T: serde_core::ser::Serialize>(mut self, data: &T) -> Self {
        let body = self.body_builder.json(data).map_err(HttpError::from);
        self.content_type = Some(crate::http_utils::CONTENT_TYPE_JSON);
        self.body_result(body)
    }

    /// Sets the request body directly.
    ///
    /// Use this when you already have an `HttpBody` instance.
    /// For most cases, prefer the more specific methods like
    /// [`text`](Self::text) or [`bytes`](Self::bytes).
    pub fn body(self, body: HttpBody) -> Self {
        self.body_result(Ok(body))
    }

    /// Sets the request body from a result that might contain an error.
    ///
    /// This is used internally by methods that might fail when creating the body.
    fn body_result(mut self, body: Result<HttpBody>) -> Self {
        self.body = Some(body);
        self
    }

    /// Creates a request with the configured settings.
    ///
    /// This method consumes the `HttpRequestBuilder` instance. It automatically sets
    /// appropriate headers based on the body, such as `Content-Length` and `Content-Type`,
    /// if they haven't been set already.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built because of errors
    /// - The URI is missing, or invalid
    /// - Body processing failed
    pub fn build(mut self) -> Result<HttpRequest> {
        let body = self.body.take().unwrap_or_else(|| Ok(self.body_builder.empty()))?;

        if let Some(length) = body.content_length() {
            try_content_length_header(&mut self.builder, length);
        }

        if let Some(content_type) = self.content_type.clone() {
            try_header(&mut self.builder, CONTENT_TYPE, content_type);
        }

        let uri = self
            .uri
            .ok_or_else(|| HttpError::validation("URI is required when building the request"))??;

        let path_and_query = uri.target_path_and_query().cloned();
        let mut request = self.builder.uri(uri.into_http_uri()?).body(body)?;
        if let Some(path_and_query) = path_and_query {
            request.extensions_mut().insert(path_and_query);
        }

        Ok(request)
    }

    /// Sets an external body implementation as the request body.
    ///
    /// This is useful when you have a custom body implementation that implements
    /// the `http_body::Body` trait and want to use it with the `HttpRequestBuilder`.
    pub fn external<B>(self, body: B) -> Self
    where
        B: http_body::Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        let body = self.body_builder.external(body);
        self.body(body)
    }

    /// Sets a streaming body for the request.
    ///
    /// This is a convenience wrapper around [`external`](Self::external) that accepts
    /// a [`Stream`] of [`BytesView`] chunks. It avoids the need to manually wrap
    /// the stream in a [`StreamBody`][http_body_util::StreamBody].
    ///
    /// Note that streaming bodies do not have a known content length, so the
    /// `Content-Length` header will not be set automatically.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpRequestBuilder};
    /// # use bytesbuf::BytesView;
    /// # fn example(body_builder: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let chunks = vec![
    ///     Ok(BytesView::copied_from_slice(b"hello ", body_builder)),
    ///     Ok(BytesView::copied_from_slice(b"world", body_builder)),
    /// ];
    /// let request = HttpRequestBuilder::new(body_builder)
    ///     .post("https://example.com/upload")
    ///     .stream(futures::stream::iter(chunks))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream<S>(self, stream: S) -> Self
    where
        S: Stream<Item = Result<BytesView>> + Send + 'static,
    {
        let body = self.body_builder.stream(stream);
        self.body(body)
    }
}

/// Extension methods for sending requests built with `HttpRequestBuilder`.
impl<R: RequestHandler> HttpRequestBuilder<'_, R> {
    /// Sends the request and fetches the response.
    ///
    /// Calling this method consumes the `HttpRequestBuilder` instance. It automatically sets
    /// appropriate headers based on the body, such as `Content-Length` and `Content-Type`,
    /// if they haven't been set already.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpError, HttpRequestBuilder, HttpResponse, RequestHandler};
    /// # async fn example<R: RequestHandler + Clone>(request_builder: HttpRequestBuilder<'_, R>) -> Result<(), HttpError> {
    /// let response: HttpResponse = request_builder.get("https://example.com").fetch().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built because of errors
    /// - The network request failed
    /// - Body processing failed
    pub fn fetch(self) -> impl Future<Output = Result<HttpResponse>> + Send {
        let handler = self.request_handler;

        match self.build() {
            Ok(request) => Either::Left(handler.execute(request)),
            Err(err) => Either::Right(ready(Err(err))),
        }
    }

    /// Sends the request and fetches the fully buffered response.
    ///
    /// Unlike [`fetch`](Self::fetch), this method reads the entire response body into
    /// memory before returning. This is useful when you need to process the entire
    /// response at once.
    ///
    /// Calling this method consumes the [`HttpRequestBuilder`] instance. It automatically sets
    /// appropriate headers based on the request body, such as `Content-Length` and `Content-Type`,
    /// if they haven't been set already.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpError, HttpRequestBuilder, HttpResponse, RequestHandler};
    /// # async fn example<R: RequestHandler + Clone>(request_builder: HttpRequestBuilder<'_, R>) -> Result<(), HttpError> {
    /// let response: HttpResponse = request_builder.get("https://example.com").fetch_buffered().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built because of errors
    /// - The network request failed
    /// - Body processing failed
    /// - The response content exceeds the size limit (default is 2 GB)
    pub async fn fetch_buffered(self) -> Result<HttpResponse> {
        let response = self.fetch().await?;

        let (parts, body) = response.into_parts();
        let body = body.into_buffered().await?;

        Ok(HttpResponse::from_parts(parts, body))
    }

    /// Sends the request and fetches the response as text.
    ///
    /// Calling this method consumes the [`HttpRequestBuilder`] instance. It automatically sets
    /// appropriate headers based on the request body, such as `Content-Length` and `Content-Type`,
    /// if they haven't been set already.
    ///
    /// # Body Processing
    ///
    /// The response body is processed as UTF-8 text. If the response body is not valid UTF-8,
    /// this method will return an error. This method returns a [`Response<String>`], where the body
    /// is the text content of the response. This preserves all the information about the response.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::Response;
    /// # use http_extensions::{HttpError, HttpRequestBuilder, HttpResponse, RequestHandler};
    /// # async fn example<R: RequestHandler + Clone>(request_builder: HttpRequestBuilder<'_, R>) -> Result<(), HttpError> {
    /// let response: Response<String> = request_builder.get("https://example.com").fetch_text().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built because of errors
    /// - The network request failed
    /// - Body processing failed
    pub async fn fetch_text(self) -> Result<Response<String>> {
        let (parts, body) = self.fetch().await?.into_parts();
        let body = body.into_text().await?;

        Ok(Response::from_parts(parts, body))
    }

    /// Sends the request and fetches the response body as a byte sequence.
    ///
    /// This is useful when working with binary data or when you need
    /// low-level access to the response bytes.
    ///
    /// Calling this method consumes the [`HttpRequestBuilder`] instance. It automatically sets
    /// appropriate headers based on the request body, such as `Content-Length` and `Content-Type`,
    /// if they haven't been set already.
    ///
    /// # Body Processing
    ///
    /// The response body is processed as a sequence of bytes. This method returns a [`Response<BytesView>`],
    /// where the body is the raw byte content of the response. This preserves all the information about the response.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::Response;
    /// # use http_extensions::{HttpError, HttpRequestBuilder, HttpResponse, RequestHandler};
    /// #
    /// # use bytesbuf::BytesView;
    /// async fn example<R: RequestHandler + Clone>(request_builder: HttpRequestBuilder<'_, R>) -> Result<(), HttpError> {
    /// let response: Response<BytesView> = request_builder.get("https://example.com").fetch_bytes().await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built because of errors
    /// - The network request failed
    /// - Body processing failed
    pub async fn fetch_bytes(self) -> Result<Response<BytesView>> {
        let (parts, body) = self.fetch().await?.into_parts();
        let body = body.into_bytes().await?;

        Ok(Response::from_parts(parts, body))
    }

    /// Sends the request and deserializes the response body as JSON.
    ///
    /// Handles the complete request-response cycle and JSON deserialization. Consumes the
    /// [`HttpRequestBuilder`] and automatically sets headers like `Content-Length` and `Content-Type`.
    /// Use this when you need owned data that can outlive the response.
    ///
    /// This method requires the `json` feature to be enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::Response;
    /// # use serde::Deserialize;
    /// # use http_extensions::{HttpError, HttpRequestBuilder, RequestHandler};
    /// #
    /// # #[derive(Deserialize)]
    /// # struct User { id: u32, name: String }
    /// #
    /// # async fn example<R: RequestHandler + Clone>(request_builder: HttpRequestBuilder<'_, R>) -> Result<(), HttpError> {
    /// let response: Response<User> = request_builder
    ///     .get("https://example.com/users/42")
    ///     .fetch_json_owned::<User>()
    ///     .await?;
    ///
    /// println!("User: {}", response.body().name);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built
    /// - The network request failed
    /// - The response body isn't valid UTF-8
    /// - JSON deserialization failed
    #[cfg(any(feature = "json", test))]
    pub async fn fetch_json_owned<J: serde_core::de::DeserializeOwned>(self) -> Result<Response<J>> {
        let (parts, body) = self.fetch().await?.into_parts();
        let body = body.into_json_owned().await?;

        Ok(Response::from_parts(parts, body))
    }

    /// Sends the request and deserializes the response body as JSON with optional borrowing.
    ///
    /// Handles the complete request-response cycle and JSON deserialization. Consumes the
    /// [`HttpRequestBuilder`] and automatically sets headers like `Content-Length` and `Content-Type`.
    /// Returns a [`Json<T>`][crate::Json] wrapper that can borrow from the underlying response data.
    ///
    /// This method requires the `json` feature to be enabled.
    ///
    /// # Note
    ///
    /// This method only prepares the data for deserialization by downloading all content
    /// to memory. The actual JSON deserialization happens lazily when you access the data
    /// through the [`Json<T>`][crate::Json] wrapper.
    ///
    /// # Examples
    ///
    /// ```
    /// # use serde::Deserialize;
    /// # use std::borrow::Cow;
    /// # use http_extensions::{HttpError, HttpRequestBuilder, Json, RequestHandler};
    /// #
    /// # #[derive(Deserialize)]
    /// # struct User<'a> { id: u32, #[serde(borrow)] name: Cow<'a, str> }
    /// #
    /// # async fn example<R: RequestHandler + Clone>(request_builder: HttpRequestBuilder<'_, R>) -> Result<(), HttpError> {
    /// let mut response: Json<User> = request_builder
    ///     .get("https://example.com/users/42")
    ///     .fetch_json::<User>()
    ///     .await?
    ///     .into_body();
    ///
    /// let user: User = response.read()?;
    /// println!("User: {}", user.name);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The request couldn't be built
    /// - The network request failed
    /// - The response body isn't valid UTF-8
    #[cfg(any(feature = "json", test))]
    pub async fn fetch_json<'de, J: serde_core::de::Deserialize<'de>>(self) -> Result<Response<crate::Json<J>>> {
        let (parts, body) = self.fetch().await?.into_parts();
        let body = body.into_json().await?;

        Ok(Response::from_parts(parts, body))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;
    use http::StatusCode;
    use http::header::CONTENT_LENGTH;
    use ohno::ErrorExt;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::http_request_builder_ext::HttpRequestBuilderExt;
    use crate::testing::{SingleChunkBody, create_stream_body_from_chunks};
    use crate::{FakeHandler, HeaderMapExt, HttpResponseBuilder, RequestExt};

    #[test]
    fn new_with_borrowed_creator() {
        let creator = HttpBodyBuilder::new_fake();
        let request_builder = HttpRequestBuilder::new(&creator);
        let request = request_builder
            .method(Method::GET)
            .uri("https://example.com")
            .text("test")
            .build()
            .unwrap();
        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "test");
    }

    #[test]
    fn json_body_ok() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .json(&JsonData { id: 42 })
            .build()
            .unwrap();
        assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, 0), 9);
        assert_eq!(request.headers().get_str_value_or(CONTENT_TYPE, ""), "application/json");
        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "{\"id\":42}");
    }

    #[test]
    fn json_does_not_override_existing_content_type() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .header(CONTENT_TYPE, "application/custom")
            .json(&JsonData { id: 42 })
            .build()
            .unwrap();

        assert_eq!(request.headers().get_str_value_or(CONTENT_TYPE, ""), "application/custom");
    }

    #[test]
    fn text_body_ok() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .text("hello")
            .build()
            .unwrap();
        assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, 0), 5);
        assert_eq!(request.headers().get_str_value_or(CONTENT_TYPE, ""), "text/plain");
        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "hello");
    }

    #[test]
    fn text_does_not_override_existing_content_type() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .header(CONTENT_TYPE, "text/custom")
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(request.headers().get_str_value_or(CONTENT_TYPE, ""), "text/custom");
    }

    #[test]
    fn method_setting() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::PUT)
            .uri("https://example.com")
            .text("hello")
            .build()
            .unwrap();
        assert_eq!(request.method(), Method::PUT);
    }

    #[test]
    fn version_setting() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .version(Version::HTTP_2)
            .text("hello")
            .build()
            .unwrap();
        assert_eq!(request.version(), Version::HTTP_2);
    }

    #[test]
    fn header_with_string_key_value() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .header("X-Custom-Header", "custom-value")
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(request.headers().get("X-Custom-Header").unwrap(), "custom-value");
    }

    #[test]
    fn header_with_header_name_value() {
        let header_name = HeaderName::from_static("x-test-header");
        let header_value = HeaderValue::from_static("test-value");

        let request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .header(header_name.clone(), header_value.clone())
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(request.headers().get(&header_name).unwrap(), &header_value);
    }

    #[test]
    fn headers_mut_access() {
        let mut request_builder = HttpRequestBuilder::new_fake();

        // Test successful access to headers_mut
        if let Some(headers) = request_builder.headers_mut() {
            headers.insert("X-Mut-Header", "mut-value".parse().unwrap());
        }

        let request = request_builder
            .method(Method::GET)
            .uri("https://example.com")
            .text("hello")
            .build()
            .unwrap();
        assert_eq!(request.headers().get("X-Mut-Header").unwrap(), "mut-value");
    }

    #[test]
    fn multiple_headers() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .header("X-Header-1", "value1")
            .header("X-Header-2", "value2")
            .header("X-Header-3", "value3")
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(request.headers().get("X-Header-1").unwrap(), "value1");
        assert_eq!(request.headers().get("X-Header-2").unwrap(), "value2");
        assert_eq!(request.headers().get("X-Header-3").unwrap(), "value3");
    }

    #[test]
    fn direct_body_setting() {
        let body = HttpBodyBuilder::new_fake().text("direct body");
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .body(body)
            .build()
            .unwrap();

        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "direct body");
    }

    #[test]
    fn chained_operations() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .version(Version::HTTP_11)
            .header("X-Custom", "value")
            .header(CONTENT_TYPE, "application/custom")
            .text("chained")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.version(), Version::HTTP_11);
        assert_eq!(request.headers().get("X-Custom").unwrap(), "value");
        assert_eq!(request.headers().get(CONTENT_TYPE).unwrap(), "application/custom");
        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "chained");
    }

    #[test]
    fn external_functionality() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body_from_chunks(&builder, &[b"custom", b" body", b" content"]);

        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .body(body)
            .build()
            .unwrap();

        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "custom body content");
    }

    #[test]
    fn external_sets_body_from_custom_body_impl() {
        let builder = HttpBodyBuilder::new_fake();

        let request = HttpRequestBuilder::new_fake()
            .post("https://example.com/upload")
            .external(SingleChunkBody::new(BytesView::copied_from_slice(b"external payload", &builder)))
            .build()
            .unwrap();

        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "external payload");
    }

    #[test]
    fn stream_sets_body_from_chunks() {
        let builder = HttpBodyBuilder::new_fake();
        let chunks: Vec<crate::Result<BytesView>> = vec![
            Ok(BytesView::copied_from_slice(b"hello ", &builder)),
            Ok(BytesView::copied_from_slice(b"streaming ", &builder)),
            Ok(BytesView::copied_from_slice(b"world", &builder)),
        ];

        let request = HttpRequestBuilder::new_fake()
            .post("https://example.com/upload")
            .stream(futures::stream::iter(chunks))
            .build()
            .unwrap();

        // Streams don't have a known content length
        assert!(request.headers().get(CONTENT_LENGTH).is_none());
        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "hello streaming world");
    }

    #[test]
    fn bytes_body_ok() {
        let builder = HttpBodyBuilder::new_fake();

        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .bytes(BytesView::copied_from_slice(b"hello", &builder))
            .build()
            .unwrap();

        assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, 0), 5);
        assert!(request.headers().get(CONTENT_TYPE).is_none());
        assert_eq!(block_on(request.into_body().into_bytes()).unwrap(), b"hello");
    }

    #[test]
    fn empty_body_ok() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .build()
            .unwrap();

        assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, -1), 0);
        assert!(request.headers().get(CONTENT_TYPE).is_none());
        assert_eq!(block_on(request.into_body().into_bytes()).unwrap().len(), 0,);
    }

    #[test]
    fn uri_required() {
        HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .text("hello")
            .build()
            .unwrap_err();
    }

    #[derive(Serialize, Deserialize, Debug)]
    struct JsonData {
        id: u32,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct BorrowedJsonData<'a> {
        id: u32,
        #[serde(borrow)]
        name: Cow<'a, str>,
        #[serde(borrow)]
        description: Cow<'a, str>,
    }

    #[test]
    fn headers_mut_returns_none_on_error() {
        let mut request_builder = HttpRequestBuilder::new_fake();

        // Force an error in the builder by adding an invalid header
        request_builder = request_builder.header("invalid\0header", "value");

        // headers_mut should return None when builder has errors
        assert!(request_builder.headers_mut().is_none());
    }

    #[test]
    fn header_multiple_calls() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .header(CONTENT_TYPE, "application/custom1")
            .header(CONTENT_TYPE, "application/custom2")
            .build()
            .unwrap();

        let headers: Vec<_> = request.headers().get_all(CONTENT_TYPE).iter().collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].to_str().unwrap(), "application/custom1");
        assert_eq!(headers[1].to_str().unwrap(), "application/custom2");
    }

    #[test]
    fn content_type_preservation() {
        let request = HttpRequestBuilder::new_fake()
            .method(Method::POST)
            .uri("https://example.com")
            .json(&JsonData { id: 42 })
            .build()
            .unwrap();

        // Both Content-Length and Content-Type should be set
        assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, 0), 9);
        assert_eq!(request.headers().get_str_value_or(CONTENT_TYPE, ""), "application/json");
    }

    #[test]
    fn request_build_error() {
        // Create an invalid header that will cause builder to fail
        let result = HttpRequestBuilder::new_fake()
            .method(Method::GET)
            .uri("https://example.com")
            .header("invalid\0header", "value")
            .build();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message(), "invalid HTTP header name");
    }

    #[test]
    fn fetch_json_borrowed_with_escaped_strings() {
        // JSON with escaped characters that should be properly deserialized into Cow
        let json_response = r#"{"id":123,"name":"John Doe","description":"A person with \"special\" characters: \n\t\\"}"#;

        let client = FakeHandler::from_sync_handler(move |_request| {
            let json_response = json_response.to_string();

            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "application/json")
                .text(json_response)
                .build()
        });

        let mut response = block_on(
            client
                .request_builder()
                .uri("https://example.com/user")
                .method(Method::GET)
                .fetch_json::<BorrowedJsonData>(),
        )
        .unwrap()
        .into_body();

        let json_data = response.read().unwrap();

        // Verify the basic fields
        assert_eq!(json_data.id, 123);
        assert_eq!(json_data.name, "John Doe");

        // Verify that escaped characters are properly decoded
        let expected_description = "A person with \"special\" characters: \n\t\\";
        assert_eq!(json_data.description, expected_description);

        assert!(matches!(json_data.name, Cow::Borrowed(_)));
        assert!(matches!(json_data.description, Cow::Owned(_)));
    }

    #[test]
    fn json_deserialization_error() {
        let client = FakeHandler::from_sync_handler(|_request| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .text("corrupted json")
                .build()
        });

        let result = block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::GET)
                .fetch_json_owned::<JsonData>(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message().contains("JSON deserialization error"));
    }

    #[test]
    fn fetch_ok() {
        let client =
            FakeHandler::from_sync_handler(|_request| HttpResponseBuilder::new_fake().status(StatusCode::OK).text("response body").build());

        let response = block_on(client.request_builder().uri("https://example.com").method(Method::GET).fetch()).unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "response body");
    }

    #[test]
    fn fetch_buffered_ok() {
        let client = FakeHandler::from_sync_handler(|_request| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .text("buffered response")
                .build()
        });

        let response = block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::GET)
                .fetch_buffered(),
        )
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "buffered response");
    }

    #[test]
    fn fetch_text_ok() {
        let client =
            FakeHandler::from_sync_handler(|_request| HttpResponseBuilder::new_fake().status(StatusCode::OK).text("text response").build());

        let response = block_on(client.request_builder().uri("https://example.com").method(Method::GET).fetch_text()).unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.into_body(), "text response");
    }

    #[test]
    fn fetch_bytes_ok() {
        let client = FakeHandler::from_sync_handler(|_request| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .bytes(BytesView::copied_from_slice(b"BytesView response", &HttpBodyBuilder::new_fake()))
                .build()
        });

        let response = block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::GET)
                .fetch_bytes(),
        )
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.into_body(), b"BytesView response");
    }

    #[test]
    fn fetch_json_ok() {
        let client = FakeHandler::from_sync_handler(|_request| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .json(&JsonData { id: 42 })
                .build()
        });

        let response = block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::GET)
                .fetch_json::<JsonData>(),
        )
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json_data = response.into_body().read().unwrap();
        assert_eq!(json_data.id, 42);
    }

    #[test]
    fn fetch_json_owned_ok() {
        let client = FakeHandler::from_sync_handler(|_request| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .json(&JsonData { id: 123 })
                .build()
        });

        let response = block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::GET)
                .fetch_json_owned::<JsonData>(),
        )
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.into_body().id, 123);
    }

    #[test]
    fn fetch_with_request_validation() {
        let client = FakeHandler::from_async_handler(|request| {
            async move {
                // Validate the request that was sent
                assert_eq!(request.method(), Method::POST);
                assert_eq!(request.headers().get_str_value_or("x-test", ""), "chained");
                assert_eq!(request.version(), http::Version::HTTP_2);
                assert_eq!(request.into_body().into_text().await.unwrap(), "chained body");

                HttpResponseBuilder::new_fake().status(StatusCode::CREATED).build()
            }
        });

        let response = block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::POST)
                .header("x-test", "chained")
                .version(http::Version::HTTP_2)
                .text("chained body")
                .fetch(),
        )
        .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[test]
    fn fetch_with_empty_body() {
        let client = FakeHandler::from_async_handler(|request| async move {
            assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, -1), 0);
            assert!(request.headers().get(CONTENT_TYPE).is_none());
            let body_len = request.into_body().into_bytes().await.unwrap().len();
            assert_eq!(body_len, 0);

            HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
        });

        block_on(client.request_builder().uri("https://example.com").method(Method::GET).fetch()).unwrap();
    }

    #[test]
    fn fetch_with_json_body_validation() {
        let client = FakeHandler::from_sync_handler(|request| {
            // Both Content-Length and Content-Type should be set
            assert_eq!(request.headers().get_value_or(CONTENT_LENGTH, 0), 9);
            assert_eq!(request.headers().get_str_value_or(CONTENT_TYPE, ""), "application/json");

            HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
        });

        block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::POST)
                .json(&JsonData { id: 42 })
                .fetch(),
        )
        .unwrap();
    }

    #[test]
    fn fetch_with_multiple_headers() {
        let client = FakeHandler::from_sync_handler(|request| {
            assert_eq!(request.headers().get_str_value_or("x-first", ""), "first");
            assert_eq!(request.headers().get_str_value_or("x-second", ""), "second");
            assert_eq!(request.version(), http::Version::HTTP_11);

            HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
        });

        block_on(
            client
                .request_builder()
                .uri("https://example.com")
                .method(Method::GET)
                .header("x-first", "first")
                .header("x-second", "second")
                .version(http::Version::HTTP_11)
                .fetch(),
        )
        .unwrap();
    }

    #[test]
    fn get_method_sets_uri_and_method() {
        let request = HttpRequestBuilder::new_fake().get("https://example.com/api").build().unwrap();

        assert_eq!(request.method(), Method::GET);
        assert_eq!(request.uri(), "https://example.com/api");
    }

    #[test]
    fn post_method_sets_uri_and_method() {
        let request = HttpRequestBuilder::new_fake()
            .post("https://example.com/api")
            .text("data")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "https://example.com/api");
    }

    #[test]
    fn delete_method_sets_uri_and_method() {
        let request = HttpRequestBuilder::new_fake()
            .delete("https://example.com/api/123")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::DELETE);
        assert_eq!(request.uri(), "https://example.com/api/123");
    }

    #[test]
    fn put_method_sets_uri_and_method() {
        let request = HttpRequestBuilder::new_fake()
            .put("https://example.com/api/123")
            .text("updated data")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::PUT);
        assert_eq!(request.uri(), "https://example.com/api/123");
    }

    #[test]
    fn patch_method_sets_uri_and_method() {
        let request = HttpRequestBuilder::new_fake()
            .patch("https://example.com/api/123")
            .text("partial update")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::PATCH);
        assert_eq!(request.uri(), "https://example.com/api/123");
    }

    #[test]
    fn head_method_sets_uri_and_method() {
        let request = HttpRequestBuilder::new_fake().head("https://example.com/api").build().unwrap();

        assert_eq!(request.method(), Method::HEAD);
        assert_eq!(request.uri(), "https://example.com/api");
        assert_eq!(request.path_and_query().unwrap().to_uri_string(), "/api");
    }

    #[test]
    fn method_convenience_functions_can_be_chained() {
        let request = HttpRequestBuilder::new_fake()
            .post("https://example.com/api")
            .header("Authorization", "Bearer token")
            .json(&JsonData { id: 42 })
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "https://example.com/api");
        assert_eq!(request.headers().get_str_value_or("Authorization", ""), "Bearer token");
        assert_eq!(block_on(request.into_body().into_text()).unwrap(), "{\"id\":42}");
    }

    #[test]
    fn method_convenience_with_fetch() {
        let client = FakeHandler::from_sync_handler(|request| {
            assert_eq!(request.method(), Method::POST);
            assert_eq!(request.uri(), "https://example.com/api");

            HttpResponseBuilder::new_fake().status(StatusCode::CREATED).build()
        });

        let response = block_on(client.request_builder().post("https://example.com/api").text("test data").fetch()).unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[test]
    fn extension_attaches_to_request() {
        use crate::UrlTemplateLabel;

        let request = HttpRequestBuilder::new_fake()
            .get("https://example.com/api/users/123")
            .extension(UrlTemplateLabel::new("/api/users/{id}"))
            .build()
            .unwrap();

        let label = request.extensions().get::<UrlTemplateLabel>().expect("extension should be present");
        assert_eq!(label.as_str(), "/api/users/{id}");
    }

    #[test]
    fn extension_with_custom_type() {
        #[derive(Clone, Debug, PartialEq)]
        struct RequestId(String);

        let request = HttpRequestBuilder::new_fake()
            .get("https://example.com/api")
            .extension(RequestId("req-123".to_string()))
            .build()
            .unwrap();

        let id = request.extensions().get::<RequestId>().expect("extension should be present");
        assert_eq!(id.0, "req-123");
    }

    #[test]
    fn fetch_returns_error_when_build_fails() {
        let handler = FakeHandler::from_sync_handler(|_request| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .text("should not reach")
                .build()
        });

        let result = block_on(handler.request_builder().method(Method::GET).fetch());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.message().contains("URI is required"),
            "expected 'URI is required' but got: {}",
            err.message()
        );
    }
}
