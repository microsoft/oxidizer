// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use bytesbuf::BytesView;
use futures::Stream;
use http::header::CONTENT_TYPE;
use http::{HeaderName, HeaderValue, Version};

use crate::http_utils::{CONTENT_TYPE_TEXT, try_content_length_header, try_header};
use crate::{HttpBody, HttpBodyBuilder, HttpError, HttpResponse, Result};

/// A fluent builder for creating HTTP responses.
///
/// `HttpResponseBuilder` simplifies the process of building HTTP responses by providing a chainable API.
/// It handles setting headers, different body types, and offers convenient methods for common
/// response handling patterns.
///
/// > **Note**: While useful in application code, `HttpResponseBuilder` is primarily designed for testing HTTP handlers
/// > and middleware. The `HttpResponseBuilder::new_fake` method makes it particularly easy to create test responses
/// > without needing a real HTTP context.
///
/// # Examples
///
/// ```
/// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponse, HttpResponseBuilder};
/// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
/// // Create a response builder
/// let response_builder = HttpResponseBuilder::new(creator);
///
/// // Customize the response builder
/// let response_builder = response_builder
///     .text("Hello world")
///     .status(200)
///     .header("X-Custom-Header", "value");
///
/// // Build the response
/// let response: HttpResponse = response_builder.build()?;
///
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[must_use]
pub struct HttpResponseBuilder<'a> {
    creator: Cow<'a, HttpBodyBuilder>,
    builder: http::response::Builder,
    body: Option<Result<HttpBody>>,
    content_type: Option<HeaderValue>,
}

impl HttpResponseBuilder<'static> {
    /// Creates a new response builder instance for testing.
    ///
    /// This method provides a convenient way to create a `HttpResponseBuilder` for tests
    /// without needing an existing body creator. The response builder is ready to be
    /// configured with headers, status, and body.
    ///
    /// The `test-util` feature must be enabled to use this method.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # fn example() -> Result<(), HttpError> {
    /// let response = HttpResponseBuilder::new_fake()
    ///     .status(200)
    ///     .text("Test response")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(any(feature = "test-util", test))]
    pub fn new_fake() -> Self {
        Self {
            creator: Cow::Owned(HttpBodyBuilder::new_fake()),
            builder: http::response::Builder::new(),
            body: None,
            content_type: None,
        }
    }
}

impl<'a> HttpResponseBuilder<'a> {
    /// Creates a new response builder instance with the given body creator.
    pub fn new(creator: &'a HttpBodyBuilder) -> Self {
        Self {
            creator: Cow::Borrowed(creator),
            builder: http::response::Builder::new(),
            body: None,
            content_type: None,
        }
    }
}

impl HttpResponseBuilder<'_> {
    /// Sets a plain text body for the response.
    ///
    /// Automatically sets the `Content-Type` header to `text/plain`.
    /// If the `Content-Type` header is already set, it will not override it.
    ///
    /// This method always encodes the provided string as UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let response_builder = HttpResponseBuilder::new(creator)
    ///     .status(200)
    ///     .text("Hello world");
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn text(mut self, data: impl AsRef<str>) -> Self {
        let body = self.creator.text(data);
        self.content_type = Some(CONTENT_TYPE_TEXT);
        self.body(body)
    }

    /// Sets a byte sequence as the response body.
    ///
    /// Use this when you need to send raw binary data.
    /// Unlike [`text`](Self::text), this doesn't set a `Content-Type` header.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # use bytesbuf::BytesView;
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// // Create a BytesView over some bytes
    /// let payload = BytesView::copied_from_slice(b"hello world", creator);
    ///
    /// let response_builder = HttpResponseBuilder::new(creator).status(200).bytes(payload);
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn bytes(self, b: impl Into<BytesView>) -> Self {
        let body = self.creator.bytes(b);
        self.body(body)
    }

    /// Sets a JSON-serialized body for the response.
    ///
    /// Takes any type that implements `serde::Serialize` and converts it to JSON with the following rules:
    ///
    /// - The `Content-Type` header is set to `application/json` if not already set.
    /// - The data is always encoded as UTF-8.
    ///
    /// This method requires the `json` feature to be enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// # use serde_json::json;
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let json_value = json!({ "id": 42, "name": "Alice" });
    /// let response_builder = HttpResponseBuilder::new(creator)
    ///     .status(200)
    ///     .json(&json_value);
    ///
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    #[cfg(any(feature = "json", test))]
    pub fn json<T: serde_core::ser::Serialize>(mut self, data: &T) -> Self {
        let body = self.creator.json(data).map_err(HttpError::from);
        self.content_type = Some(crate::http_utils::CONTENT_TYPE_JSON);
        self.body_result(body)
    }

    /// Sets the HTTP status code for the response.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # use http::StatusCode;
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let response_builder = HttpResponseBuilder::new(creator)
    ///     .status(StatusCode::OK)
    ///     .status(200); // You can also use integers
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn status(mut self, status: impl TryInto<http::StatusCode, Error: Into<http::Error>>) -> Self {
        self.builder = self.builder.status(status);
        self
    }

    /// Sets the HTTP protocol version for the response.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # use http::Version;
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let response_builder = HttpResponseBuilder::new(creator).version(Version::HTTP_2);
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn version(mut self, version: Version) -> Self {
        self.builder = self.builder.version(version);
        self
    }

    /// Adds an extension to the response.
    ///
    /// Extensions are type-mapped data that can be attached to responses for use by
    /// middleware, handlers, or other parts of your application.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpResponseBuilder;
    /// #[derive(Clone)]
    /// struct RequestId(String);
    ///
    /// let response = HttpResponseBuilder::new_fake()
    ///     .status(200)
    ///     .extension(RequestId("req-123".to_string()))
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

    /// Adds a header to the response.
    ///
    /// This method accepts any type that can be converted to a [`HeaderName`] and [`HeaderValue`].
    /// It returns `self` to enable method chaining.
    ///
    /// # Performance
    ///
    /// It's better to use pre-created `HeaderName` and `HeaderValue` instances to avoid
    /// parsing overhead. This applies for values that are fixed and used multiple times.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http::header::CACHE_CONTROL;
    /// # use http::HeaderValue;
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// // Pre-create a HeaderValue to avoid parsing overhead
    /// let header_value = HeaderValue::from_static("no-cache");
    ///
    /// let response_builder = HttpResponseBuilder::new(creator)
    ///     .header(CACHE_CONTROL, header_value.clone()) // Using a pre-created HeaderValue
    ///     .header("X-Custom-Header", "value");
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn header(
        mut self,
        key: impl TryInto<HeaderName, Error: Into<http::Error>>,
        value: impl TryInto<HeaderValue, Error: Into<http::Error>>,
    ) -> Self {
        self.builder = self.builder.header(key, value);
        self
    }

    /// Provides mutable access to the response headers.
    ///
    /// Use this when you need to manipulate headers directly.
    /// For simple header addition, prefer using the [`header`](Self::header) method.
    ///
    /// When the builder has errors, this method will return `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let mut response_builder = HttpResponseBuilder::new(creator);
    ///
    /// if let Some(headers) = response_builder.headers_mut() {
    ///     headers.insert("X-Custom-Header", "value".parse().unwrap());
    /// }
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn headers_mut(&mut self) -> Option<&mut http::HeaderMap<HeaderValue>> {
        self.builder.headers_mut()
    }

    /// Sets the response body directly.
    ///
    /// Use this when you already have an `HttpBody` instance.
    /// For most cases, prefer the more specific methods like
    /// [`text`](Self::text) or [`bytes`](Self::bytes).
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # use http_extensions::HttpBody;
    /// # fn example(creator: &HttpBodyBuilder, custom_body: HttpBody) -> Result<(), HttpError> {
    /// let response_builder = HttpResponseBuilder::new(creator).body(custom_body);
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn body(self, body: HttpBody) -> Self {
        self.body_result(Ok(body))
    }

    /// Sets the response body from a result that might contain an error.
    ///
    /// This is used internally by methods that might fail when creating the body.
    fn body_result(mut self, body: Result<HttpBody>) -> Self {
        self.body = Some(body);
        self
    }

    /// Creates a response with the configured settings.
    ///
    /// This method consumes the `HttpResponseBuilder` instance. It automatically sets
    /// appropriate headers based on the body, such as `Content-Length` and `Content-Type`,
    /// if they haven't been set already.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponse, HttpResponseBuilder};
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let response: HttpResponse = HttpResponseBuilder::new(creator)
    ///     .status(200)
    ///     .text("Success")
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The response couldn't be built because of errors
    /// - Body processing failed
    pub fn build(mut self) -> Result<HttpResponse> {
        let body = self.body.take().unwrap_or_else(|| Ok(self.creator.empty()))?;

        if let Some(length) = body.content_length() {
            try_content_length_header(&mut self.builder, length);
        }

        if let Some(content_type) = self.content_type.take() {
            try_header(&mut self.builder, CONTENT_TYPE, content_type);
        }

        let body = self.builder.body(body)?;

        Ok(body)
    }

    /// Sets an external body implementation as the response body.
    ///
    /// This is useful when you have a custom body implementation that implements
    /// the `http_body::Body` trait and want to use it with the `HttpResponseBuilder`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::pin::Pin;
    /// # use std::task::{Context, Poll};
    /// # use http_body::Body;
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # use bytesbuf::BytesView;
    /// // Your custom body type
    /// #[derive(Default)]
    /// struct CustomBody;
    ///
    /// // Implement the `Body` trait for your custom body type.
    /// impl Body for CustomBody {
    ///     type Data = BytesView;
    ///     type Error = HttpError;
    ///
    ///     fn poll_frame(
    ///         self: Pin<&mut Self>,
    ///         _cx: &mut Context<'_>,
    ///     ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
    ///         // Implementation details...
    ///         Poll::Ready(None)
    ///     }
    /// }
    ///
    /// # fn example(creator: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let response_builder = HttpResponseBuilder::new(creator)
    ///     .status(200)
    ///     .external(CustomBody::default());
    /// # Ok(())
    /// # }
    /// ```
    pub fn external<B>(self, body: B) -> Self
    where
        B: http_body::Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        let body = self.creator.external(body);
        self.body(body)
    }

    /// Sets a streaming body for the response.
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
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpResponseBuilder};
    /// # use bytesbuf::BytesView;
    /// # fn example(body_builder: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let chunks = vec![
    ///     Ok(BytesView::copied_from_slice(b"hello ", body_builder)),
    ///     Ok(BytesView::copied_from_slice(b"world", body_builder)),
    /// ];
    /// let response = HttpResponseBuilder::new(body_builder)
    ///     .status(200)
    ///     .stream(futures::stream::iter(chunks))
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream<S>(self, stream: S) -> Self
    where
        S: Stream<Item = Result<BytesView>> + Send + 'static,
    {
        let body = self.creator.stream(stream);
        self.body(body)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;
    use http::StatusCode;
    use http::header::CONTENT_LENGTH;
    use serde::Serialize;

    use super::*;
    use crate::HeaderMapExt;
    use crate::testing::{SingleChunkBody, create_stream_body_from_chunks};

    #[test]
    fn new_with_borrowed_creator() {
        let creator = HttpBodyBuilder::new_fake();
        let response_builder = HttpResponseBuilder::new(&creator);
        let response = response_builder.text("test").build().unwrap();
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "test");
    }

    #[test]
    fn json_body_ok() {
        let response = HttpResponseBuilder::new_fake().json(&JsonData { id: 42 }).build().unwrap();
        assert_eq!(response.headers().get_value_or(CONTENT_LENGTH, 0), 9);
        assert_eq!(response.headers().get_str_value_or(CONTENT_TYPE, ""), "application/json");
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "{\"id\":42}");
    }

    #[test]
    fn json_does_not_override_existing_content_type() {
        let response = HttpResponseBuilder::new_fake()
            .header(CONTENT_TYPE, "application/custom")
            .json(&JsonData { id: 42 })
            .build()
            .unwrap();

        assert_eq!(response.headers().get_str_value_or(CONTENT_TYPE, ""), "application/custom");
    }

    #[test]
    fn text_body_ok() {
        let response = HttpResponseBuilder::new_fake().text("hello").build().unwrap();
        assert_eq!(response.headers().get_value_or(CONTENT_LENGTH, 0), 5);
        assert_eq!(response.headers().get_str_value_or(CONTENT_TYPE, ""), "text/plain");
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "hello");
    }

    #[test]
    fn text_does_not_override_existing_content_type() {
        let response = HttpResponseBuilder::new_fake()
            .header(CONTENT_TYPE, "text/custom")
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(response.headers().get_str_value_or(CONTENT_TYPE, ""), "text/custom");
    }

    #[test]
    fn status_with_status_code() {
        let response = HttpResponseBuilder::new_fake()
            .status(StatusCode::NOT_FOUND)
            .text("not found")
            .build()
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn version_setting() {
        let response = HttpResponseBuilder::new_fake()
            .version(Version::HTTP_2)
            .text("hello")
            .build()
            .unwrap();
        assert_eq!(response.version(), Version::HTTP_2);
    }

    #[test]
    fn header_with_string_key_value() {
        let response = HttpResponseBuilder::new_fake()
            .header("X-Custom-Header", "custom-value")
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(response.headers().get("X-Custom-Header").unwrap(), "custom-value");
    }

    #[test]
    fn header_with_header_name_value() {
        let header_name = HeaderName::from_static("x-test-header");
        let header_value = HeaderValue::from_static("test-value");

        let response = HttpResponseBuilder::new_fake()
            .header(header_name.clone(), header_value.clone())
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(response.headers().get(&header_name).unwrap(), &header_value);
    }

    #[test]
    fn headers_mut_access() {
        let mut response_builder = HttpResponseBuilder::new_fake();

        // Test successful access to headers_mut
        if let Some(headers) = response_builder.headers_mut() {
            headers.insert("X-Mut-Header", "mut-value".parse().unwrap());
        }

        let response = response_builder.text("hello").build().unwrap();
        assert_eq!(response.headers().get("X-Mut-Header").unwrap(), "mut-value");
    }

    #[test]
    fn multiple_headers() {
        let response = HttpResponseBuilder::new_fake()
            .header("X-Header-1", "value1")
            .header("X-Header-2", "value2")
            .header("X-Header-3", "value3")
            .text("hello")
            .build()
            .unwrap();

        assert_eq!(response.headers().get("X-Header-1").unwrap(), "value1");
        assert_eq!(response.headers().get("X-Header-2").unwrap(), "value2");
        assert_eq!(response.headers().get("X-Header-3").unwrap(), "value3");
    }

    #[test]
    fn direct_body_setting() {
        let body = HttpBodyBuilder::new_fake().text("direct body");
        let response = HttpResponseBuilder::new_fake().body(body).build().unwrap();

        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "direct body");
    }

    #[test]
    fn chained_operations() {
        let response = HttpResponseBuilder::new_fake()
            .status(201)
            .version(Version::HTTP_11)
            .header("X-Custom", "value")
            .header(CONTENT_TYPE, "application/custom")
            .text("chained")
            .build()
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.version(), Version::HTTP_11);
        assert_eq!(response.headers().get("X-Custom").unwrap(), "value");
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "application/custom");
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "chained");
    }

    #[test]
    fn external_functionality() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body_from_chunks(&builder, &[b"custom", b" body", b" content"]);

        let response = HttpResponseBuilder::new_fake().body(body).build().unwrap();

        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "custom body content");
    }

    #[test]
    fn external_sets_body_from_custom_body_impl() {
        let builder = HttpBodyBuilder::new_fake();

        let response = HttpResponseBuilder::new_fake()
            .status(200)
            .external(SingleChunkBody::new(BytesView::copied_from_slice(b"external payload", &builder)))
            .build()
            .unwrap();

        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "external payload");
    }

    #[test]
    fn stream_sets_body_from_chunks() {
        let builder = HttpBodyBuilder::new_fake();
        let chunks: Vec<crate::Result<BytesView>> = vec![
            Ok(BytesView::copied_from_slice(b"hello ", &builder)),
            Ok(BytesView::copied_from_slice(b"streaming ", &builder)),
            Ok(BytesView::copied_from_slice(b"world", &builder)),
        ];

        let response = HttpResponseBuilder::new_fake()
            .status(200)
            .stream(futures::stream::iter(chunks))
            .build()
            .unwrap();

        // Streams don't have a known content length
        assert!(response.headers().get(CONTENT_LENGTH).is_none());
        assert_eq!(block_on(response.into_body().into_text()).unwrap(), "hello streaming world");
    }

    #[test]
    fn bytes_body_ok() {
        let builder = HttpBodyBuilder::new_fake();

        let response = HttpResponseBuilder::new_fake()
            .bytes(BytesView::copied_from_slice(b"hello", &builder))
            .build()
            .unwrap();

        assert_eq!(response.headers().get_value_or(CONTENT_LENGTH, 0), 5);
        assert!(response.headers().get(CONTENT_TYPE).is_none());
        assert_eq!(block_on(response.into_body().into_bytes()).unwrap(), b"hello");
    }

    #[test]
    fn empty_body_ok() {
        let response = HttpResponseBuilder::new_fake().build().unwrap();

        assert_eq!(response.headers().get_value_or(CONTENT_LENGTH, -1), 0);
        assert!(response.headers().get(CONTENT_TYPE).is_none());
        assert_eq!(block_on(response.into_body().into_bytes()).unwrap().len(), 0,);
    }

    #[test]
    fn content_length_set_to_zero_when_body_has_no_length() {
        // Create a body without a known content length
        let body = HttpBodyBuilder::new_fake().empty();
        let response = HttpResponseBuilder::new_fake().body(body).build().unwrap();

        // Content-Length should be set to 0 for empty body
        assert_eq!(response.headers().get_value_or(CONTENT_LENGTH, -1), 0);
    }

    #[test]
    fn bytes_with_different_data() {
        let data = b"binary data";
        let payload = BytesView::copied_from_slice(data, &HttpBodyBuilder::new_fake());

        let response = HttpResponseBuilder::new_fake().bytes(payload).build().unwrap();

        assert_eq!(response.headers().get_value_or(CONTENT_LENGTH, 0), data.len() as u64);
    }

    #[derive(Serialize, Debug)]
    struct JsonData {
        id: u32,
    }

    #[test]
    fn extension_attaches_to_response() {
        #[derive(Clone, Debug, PartialEq)]
        struct RequestId(String);

        let response = HttpResponseBuilder::new_fake()
            .status(200)
            .extension(RequestId("req-123".to_string()))
            .build()
            .unwrap();

        let id = response.extensions().get::<RequestId>().expect("extension should be present");
        assert_eq!(id.0, "req-123");
    }

    #[test]
    fn extension_with_multiple_types() {
        #[derive(Clone, Debug)]
        struct RequestId(String);
        #[derive(Clone, Debug)]
        struct TraceId(u64);

        let response = HttpResponseBuilder::new_fake()
            .status(200)
            .extension(RequestId("req-456".to_string()))
            .extension(TraceId(12345))
            .build()
            .unwrap();

        let request_id = response.extensions().get::<RequestId>().unwrap();
        let trace_id = response.extensions().get::<TraceId>().unwrap();

        assert_eq!(request_id.0, "req-456");
        assert_eq!(trace_id.0, 12345);
    }
}
