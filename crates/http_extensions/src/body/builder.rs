// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};
use futures::{Stream, TryStreamExt};
use http_body::{Body, Frame};
use http_body_util::BodyExt;
use thread_aware::{ThreadAware, Unaware};
use tick::Clock;

#[cfg(any(feature = "json", test))]
use crate::json::JsonError;
use crate::{HttpError, Result};

use super::timeout_body::TimeoutBody;
use super::{HttpBody, Kind};

/// Builder for creating optimized HTTP bodies.
///
/// This builder optimizes memory usage and performance for HTTP bodies by providing:
///
/// - Memory pooling for better performance
/// - Runtime-specific optimizations
/// - Easier testing with the `test-util` feature
///
/// # Examples
///
/// ```
/// # use http_extensions::{HttpBody, HttpBodyBuilder};
/// # fn example(create_body: &HttpBodyBuilder) {
/// // Create different body types
/// let text_body: HttpBody = create_body.text("Hello world");
/// let empty_body: HttpBody = create_body.empty();
/// let binary_body: HttpBody = create_body.slice(&[1, 2, 3, 4]);
/// # }
/// ```
///
/// # Testing
///
/// With the `test-util` feature enabled, you can create a test instance using `HttpBodyBuilder::new_fake()`.
#[derive(Debug, Clone, ThreadAware)]
pub struct HttpBodyBuilder {
    memory: MemoryWrapper,
    pub(super) response_buffer_limit: Option<usize>,
}

impl HttpBodyBuilder {
    /// Creates a test-friendly [`HttpBodyBuilder`] instance.
    ///
    /// Useful for unit tests. Available with the `test-util` feature, this allows creating and
    /// working with HTTP bodies without any real network or server setup.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpBodyBuilder;
    /// let builder = HttpBodyBuilder::new_fake();
    /// let text_body = builder.text("Test content");
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_fake() -> Self {
        // We use a default response buffer limit of `2GB` for fake instances.
        Self::new(GlobalPool::new())
    }

    /// Creates a new instance of [`HttpBodyBuilder`].
    ///
    /// This method uses a per-thread memory pool from [`GlobalPool`].
    #[must_use]
    pub fn new(memory: GlobalPool) -> Self {
        Self {
            memory: MemoryWrapper::Global(memory),
            response_buffer_limit: None,
        }
    }

    /// Creates a new instance of [`HttpBodyBuilder`] with custom memory.
    ///
    /// When using this method, the memory is shared across all threads as opposed
    /// to the global per-thread memory used by [`HttpBodyBuilder::new`].
    #[must_use]
    pub fn with_custom_memory(memory: impl MemoryShared) -> Self {
        Self {
            memory: MemoryWrapper::Opaque(Unaware(OpaqueMemory::new(memory))),
            response_buffer_limit: None,
        }
    }

    /// Sets the response buffer limit for all bodies created by this builder.
    #[must_use]
    pub const fn with_response_buffer_limit(mut self, limit: Option<usize>) -> Self {
        self.response_buffer_limit = limit;
        self
    }

    /// Creates an `HttpBody` from any custom body implementation.
    ///
    /// Use this to integrate custom types that implement [`http_body::Body`] with the
    /// [`HttpBody`] system. Useful for third-party libraries or your own custom body
    /// implementations.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpBody};
    /// # use http_body::Body;
    /// # use std::pin::Pin;
    /// # use std::task::{Context, Poll};
    /// # use bytesbuf::BytesView;
    /// #
    /// // Your custom body type
    /// struct CustomBody(Vec<u8>);
    ///
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
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// // Create HttpBody from your custom body
    /// let custom_body = CustomBody(vec![1, 2, 3, 4]);
    /// let body = create_body.custom_body(custom_body);
    /// # }
    /// ```
    pub fn custom_body<B>(&self, body: B) -> HttpBody
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        let body = body.map_err(Into::into);
        HttpBody::new(Kind::Body(Box::pin(body)), self.clone())
    }

    /// Creates an `HttpBody` from a custom body implementation with a total download timeout.
    ///
    /// This behaves like [`custom_body`][Self::custom_body] but enforces a deadline on the
    /// entire data reception. If the body is not fully received within the specified duration
    /// a timeout error is returned.
    ///
    /// The deadline is computed once at construction time; successive polls see a shrinking
    /// remaining time rather than a fixed per-poll timeout.
    pub fn custom_body_with_timeout<B>(&self, body: B, timeout: std::time::Duration, clock: &Clock) -> HttpBody
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        let body = body.map_err(Into::into);

        // check that the timeout is valid (i.e. the deadline does not overflow)
        match clock.instant().checked_add(timeout) {
            Some(deadline) => HttpBody::new(Kind::Body(Box::pin(TimeoutBody::new(body, deadline, timeout, clock))), self.clone()),
            None => self.custom_body(body),
        }
    }

    /// Use [`custom_body`][Self::custom_body] instead.
    #[deprecated(note = "use `custom_body` instead")]
    #[doc(hidden)]
    pub fn external<B>(&self, body: B) -> HttpBody
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        self.custom_body(body)
    }

    /// Creates a body from a stream of byte chunks.
    ///
    /// Accepts a [`Stream`][futures::Stream] of [`BytesView`] chunks and creates a streaming
    /// body from them.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError};
    /// # use bytesbuf::BytesView;
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// let chunks = vec![
    ///     Ok(BytesView::copied_from_slice(b"hello ", create_body)),
    ///     Ok(BytesView::copied_from_slice(b"world", create_body)),
    /// ];
    /// let body = create_body.stream(futures::stream::iter(chunks));
    ///
    /// assert_eq!(body.content_length(), None); // unknown length for streams
    /// # }
    /// ```
    pub fn stream<S>(&self, stream: S) -> HttpBody
    where
        S: Stream<Item = Result<BytesView>> + Send + 'static,
    {
        use http_body_util::StreamBody;

        let framed = stream.map_ok(Frame::data);
        self.custom_body(StreamBody::new(framed))
    }

    /// Creates a body from a stream of byte chunks with a total download timeout.
    ///
    /// This behaves like [`stream`][Self::stream] but enforces a deadline on the entire
    /// data reception. If the stream is not fully consumed within the specified duration
    /// a timeout error is returned.
    ///
    /// The deadline is computed once at construction time; successive polls see a shrinking
    /// remaining time rather than a fixed per-poll timeout.
    pub fn stream_with_timeout<S>(&self, stream: S, timeout: std::time::Duration, clock: &Clock) -> HttpBody
    where
        S: Stream<Item = Result<BytesView>> + Send + 'static,
    {
        use http_body_util::StreamBody;

        let framed = stream.map_ok(Frame::data);
        self.custom_body_with_timeout(StreamBody::new(framed), timeout, clock)
    }

    /// Creates a body from text.
    ///
    /// Works with both string literals and `String` types.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpBody};
    /// #
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// let body1 = create_body.text("Hello, world!"); // From &str
    /// let body2 = create_body.text(String::from("Hello, world!")); // From String
    ///
    /// assert_eq!(body1.content_length(), body2.content_length());
    /// # }
    /// ```
    pub fn text(&self, str: impl AsRef<str>) -> HttpBody {
        self.slice(str.as_ref().as_bytes())
    }

    /// Creates a body from a slice of bytes.
    ///
    /// Use this when you have a single slice of raw bytes that needs to be sent in a request or
    /// response.
    ///
    /// # Performance
    ///
    /// This will copy the contents of the byte slice. For more efficiency, you should consider
    /// using [`bytes()`][Self::bytes].
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpBodyBuilder;
    /// #
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// // "Hello" in ASCII
    /// let data = [0x48, 0x65, 0x6C, 0x6C, 0x6F];
    /// let body = create_body.slice(&data);
    ///
    /// assert_eq!(body.content_length(), Some(5));
    /// # }
    /// ```
    pub fn slice(&self, data: impl AsRef<[u8]>) -> HttpBody {
        let mut builder = self.reserve(data.as_ref().len());
        builder.put_slice(data.as_ref());
        self.bytes(builder.consume_all())
    }

    /// Creates a body from an existing `BytesView`.
    ///
    /// Use this when you already have a `BytesView` and want to use it as an HTTP body.
    ///
    /// # Performance
    ///
    /// This method does not copy the contents of the [`BytesView`],
    /// providing greater efficiency compared to [`slice()`][Self::slice].
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpBodyBuilder;
    /// # use bytesbuf::BytesView;
    /// #
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// // Create a body from existing bytes of data
    /// let body = create_body.bytes(BytesView::new());
    /// assert_eq!(body.content_length(), Some(0));
    /// # }
    /// ```
    pub fn bytes(&self, b: impl Into<BytesView>) -> HttpBody {
        HttpBody::new(Kind::Bytes(Some(b.into())), self.clone())
    }

    /// Creates an empty body (zero bytes).
    ///
    /// Use this for HTTP methods that don't need a body like GET or HEAD requests,
    /// or for responses that only need status codes or headers.
    pub fn empty(&self) -> HttpBody {
        HttpBody::new(Kind::Empty, self.clone())
    }

    #[cfg(any(feature = "json", test))]
    /// Creates a body from a JSON-serializable value.
    ///
    /// Automatically handles serialization and creates an HTTP body ready to send.
    /// Available with the `json` feature.
    ///
    /// # Errors
    ///
    /// Returns an error if the JSON serialization fails.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError};
    /// # use serde::Serialize;
    /// #
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    /// }
    ///
    /// # fn example(create_body: &HttpBodyBuilder) -> Result<(), HttpError> {
    /// let user = User {
    ///     id: 1,
    ///     name: String::from("Alice"),
    /// };
    ///
    /// // Create a body containing the JSON representation of user
    /// let body = create_body.json(&user)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn json<T: serde_core::ser::Serialize>(&self, data: &T) -> std::result::Result<HttpBody, JsonError> {
        let builder = BytesBuf::new();
        let mut writer = builder.into_writer(&self);

        serde_json::to_writer(&mut writer, data).map_err(JsonError::serialization)?;

        Ok(self.bytes(writer.into_inner().consume_all()))
    }
}

impl Memory for HttpBodyBuilder {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

impl HasMemory for HttpBodyBuilder {
    fn memory(&self) -> impl MemoryShared {
        self.memory.clone()
    }
}

#[derive(Debug, Clone, ThreadAware)]
enum MemoryWrapper {
    Global(GlobalPool),
    Opaque(Unaware<OpaqueMemory>),
}

impl Memory for MemoryWrapper {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        match self {
            Self::Global(pool) => pool.reserve(min_bytes),
            Self::Opaque(memory) => memory.reserve(min_bytes),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use bytesbuf::mem::testing::TransparentMemory;
    use futures::executor::block_on;
    use futures::stream;
    use serde::Serialize;
    use static_assertions::assert_impl_all;
    use tick::ClockControl;

    use super::*;
    use crate::testing::{create_stream_body, create_stream_body_from_chunks};

    #[test]
    fn assert_send_and_sync() {
        assert_impl_all!(HttpBodyBuilder: Send, Sync, std::fmt::Debug);
    }

    #[test]
    fn new_with_global_memory() {
        let memory = GlobalPool::new();
        let builder = HttpBodyBuilder::new(memory);
        let body = builder.text("test");
        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn with_custom_memory() {
        let builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new());
        let body = builder.text("hello");
        let data = BytesView::try_from(body).unwrap();
        assert_eq!(data.len(), 5);
    }

    #[test]
    fn response_buffer_limit_with_some() {
        let builder = HttpBodyBuilder::new_fake().with_response_buffer_limit(Some(1024));
        assert_eq!(builder.response_buffer_limit, Some(1024));
    }

    #[test]
    fn response_buffer_limit_with_none() {
        let builder = HttpBodyBuilder::new_fake().with_response_buffer_limit(None);
        assert_eq!(builder.response_buffer_limit, None);
    }

    #[test]
    fn has_memory_returns_usable_provider() {
        let builder = HttpBodyBuilder::new_fake();
        let memory = builder.memory();
        let buf = memory.reserve(64);
        assert!(buf.capacity() >= 64);
    }

    // ── Body creation methods ────────────────────────────────────────────

    #[test]
    fn text_from_str() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("hello world");
        assert_eq!(body.content_length(), Some(11));
        let result = block_on(body.into_text()).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn text_from_string() {
        let builder = HttpBodyBuilder::new_fake();
        let text = String::from("hello world");
        let body = builder.text(text);
        assert_eq!(body.content_length(), Some(11));
        let result = block_on(body.into_text()).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn slice_creation() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.slice([1, 2, 3, 4]);
        assert_eq!(body.content_length(), Some(4));
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, &[1, 2, 3, 4]);
    }

    #[test]
    fn bytes_from_bytes_view() {
        let memory = GlobalPool::new();
        let builder = HttpBodyBuilder::new_fake();
        let bytes = BytesView::copied_from_slice(b"test", &memory);
        let body = builder.bytes(bytes);
        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn bytes_from_bytes_crate() {
        let builder = HttpBodyBuilder::new_fake();
        // `Bytes` can .into() `BytesView`
        let body = builder.bytes(Bytes::from_static(b"test"));
        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn empty_body_creation() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.empty();
        assert_eq!(body.content_length(), Some(0));
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes.len(), 0);
    }

    #[test]
    fn content_length_empty_body() {
        let builder = HttpBodyBuilder::new_fake();
        assert_eq!(builder.empty().content_length(), Some(0));
    }

    #[test]
    fn content_length_text_body() {
        let builder = HttpBodyBuilder::new_fake();
        assert_eq!(builder.text("hello").content_length(), Some(5));
    }

    #[test]
    fn content_length_slice_body() {
        let builder = HttpBodyBuilder::new_fake();
        assert_eq!(builder.slice([1, 2, 3]).content_length(), Some(3));
    }

    #[test]
    fn content_length_bytes_body() {
        let builder = HttpBodyBuilder::new_fake();
        assert_eq!(builder.bytes(BytesView::new()).content_length(), Some(0));
    }

    // ── Stream body creation ─────────────────────────────────────────────

    #[test]
    fn stream_body_creation() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body_from_chunks(&builder, &[b"hello ", b"world"]);
        assert_eq!(body.content_length(), None);
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn stream_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"");
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn stream_with_timeout_returns_data_before_deadline() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new_fake();
        let chunks: Vec<Result<BytesView>> = [b"hello " as &[u8], b"world"]
            .iter()
            .map(|c| Ok(BytesView::copied_from_slice(c, &builder)))
            .collect();
        let body = builder.stream_with_timeout(stream::iter(chunks), Duration::from_secs(30), &clock);
        assert_eq!(body.content_length(), None);
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn custom_body_with_timeout_falls_back_when_deadline_overflows() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.custom_body_with_timeout(
            http_body_util::Full::new(BytesView::copied_from_slice(b"hello", &builder)),
            Duration::MAX,
            &clock,
        );
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"hello");
    }

    // ── Deprecated methods ───────────────────────────────────────────────

    #[allow(deprecated)]
    #[test]
    fn external_delegates_to_custom_body() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.external(http_body_util::Full::new(BytesView::copied_from_slice(b"test", &builder)));
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"test");
    }

    // ── JSON body creation ───────────────────────────────────────────────

    #[test]
    fn json_serialization_makes_few_memory_allocations() {
        #[derive(Serialize)]
        struct LargePayload {
            items: Vec<Item>,
        }

        #[derive(Serialize)]
        struct Item {
            id: u32,
            name: String,
            description: String,
            value: f64,
        }

        let payload = LargePayload {
            items: (0..300)
                .map(|i| Item {
                    id: i,
                    name: format!("item-name-{i:04}"),
                    description: format!("This is a longer description for item number {i:04}"),
                    value: f64::from(i) * 1.5,
                })
                .collect(),
        };

        let expected_size = serde_json::to_vec(&payload).unwrap().len();
        assert!(
            expected_size > 25_000 && expected_size < 40_000,
            "expected ~30 KB JSON, got {expected_size} bytes"
        );

        let builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new());
        let body = builder.json(&payload).unwrap();
        let bytes_view = body.into_bytes_no_buffering().unwrap();

        assert_eq!(bytes_view.len(), expected_size);

        let block_count = bytes_view.slices().count();
        assert!(
            block_count <= 5,
            "expected at most 5 memory blocks for ~30 KB JSON serialization, got {block_count}"
        );
    }
}
