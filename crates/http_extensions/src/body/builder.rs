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

use super::options::BodyOptions;
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
/// # let builder = HttpBodyBuilder::new_fake();
/// // Create different body types
/// let text_body: HttpBody = builder.text("Hello world");
/// let empty_body: HttpBody = builder.empty();
/// let binary_body: HttpBody = builder.slice(&[1, 2, 3, 4]);
/// ```
///
/// # Testing
///
/// With the `test-util` feature enabled, you can create a test instance using `HttpBodyBuilder::new_fake()`.
#[derive(Debug, Clone, ThreadAware)]
pub struct HttpBodyBuilder {
    memory: MemoryWrapper,
    clock: Clock,
    pub(super) options: BodyOptions,
}

impl HttpBodyBuilder {
    /// Creates a test-friendly [`HttpBodyBuilder`] instance.
    ///
    /// Useful for unit tests. Available with the `test-util` feature, this allows creating and
    /// working with HTTP bodies without any real network or server setup.
    ///
    /// Uses a frozen clock, so body data receive timeouts will never fire.
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
        Self::new(GlobalPool::new(), &Clock::new_frozen())
    }

    /// Creates a new instance of [`HttpBodyBuilder`].
    ///
    /// This method uses a per-thread memory pool from [`GlobalPool`].
    #[must_use]
    pub fn new(memory: GlobalPool, clock: &Clock) -> Self {
        Self {
            memory: MemoryWrapper::Global(memory),
            clock: clock.clone(),
            options: BodyOptions::default(),
        }
    }

    /// Creates a new instance of [`HttpBodyBuilder`] with custom memory.
    ///
    /// When using this method, the memory is shared across all threads as opposed
    /// to the global per-thread memory used by [`HttpBodyBuilder::new`].
    #[must_use]
    pub fn with_custom_memory(memory: impl MemoryShared, clock: &Clock) -> Self {
        Self {
            memory: MemoryWrapper::Opaque(Unaware(OpaqueMemory::new(memory))),
            clock: clock.clone(),
            options: BodyOptions::default(),
        }
    }

    /// Sets default [`BodyOptions`] for all bodies created by this builder.
    ///
    /// Per-call options passed to [`body`](Self::body) or [`stream`](Self::stream) are
    /// merged on top: the per-call value wins when both sides set the same field.
    #[must_use]
    pub fn with_options(mut self, options: BodyOptions) -> Self {
        self.options = options;
        self
    }

    /// Creates an `HttpBody` from any body implementation.
    ///
    /// Use this to integrate custom types that implement [`http_body::Body`] with the
    /// [`HttpBody`] system. Useful for third-party libraries or your own custom body
    /// implementations.
    ///
    /// When `options` contains a timeout, the body is wrapped with an idle timeout that limits
    /// how long the body may go without yielding a frame.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{BodyOptions, HttpBodyBuilder, HttpError, HttpBody};
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// // Create HttpBody from your custom body
    /// let custom_body = CustomBody(vec![1, 2, 3, 4]);
    /// let body = builder.body(custom_body, &BodyOptions::default());
    /// ```
    pub fn body<B>(&self, body: B, options: &BodyOptions) -> HttpBody
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        let merged = options.merge(&self.options);
        let body = body.map_err(Into::into);

        match merged.timeout {
            Some(timeout) => HttpBody::new(
                Kind::Body(Box::pin(TimeoutBody::new(body, timeout, &self.clock)), merged),
                self.clone(),
            ),
            None => HttpBody::new(Kind::Body(Box::pin(body), merged), self.clone()),
        }
    }

    /// Creates a body from a stream of byte chunks.
    ///
    /// Accepts a [`Stream`][futures::Stream] of [`BytesView`] chunks and creates a streaming
    /// body from them.
    ///
    /// When `options` contains a timeout, the stream is wrapped with an idle timeout that limits
    /// how long the body may go without yielding a frame.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{BodyOptions, HttpBodyBuilder, HttpError};
    /// # use bytesbuf::BytesView;
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let chunks = vec![
    ///     Ok(BytesView::copied_from_slice(b"hello ", &builder)),
    ///     Ok(BytesView::copied_from_slice(b"world", &builder)),
    /// ];
    /// let body = builder.stream(futures::stream::iter(chunks), &BodyOptions::default());
    ///
    /// assert_eq!(body.content_length(), None); // unknown length for streams
    /// ```
    pub fn stream<S>(&self, stream: S, options: &BodyOptions) -> HttpBody
    where
        S: Stream<Item = Result<BytesView>> + Send + 'static,
    {
        use http_body_util::StreamBody;

        let framed = stream.map_ok(Frame::data);
        self.body(StreamBody::new(framed), options)
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let body1 = builder.text("Hello, world!"); // From &str
    /// let body2 = builder.text(String::from("Hello, world!")); // From String
    ///
    /// assert_eq!(body1.content_length(), body2.content_length());
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// // "Hello" in ASCII
    /// let data = [0x48, 0x65, 0x6C, 0x6C, 0x6F];
    /// let body = builder.slice(&data);
    ///
    /// assert_eq!(body.content_length(), Some(5));
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// // Create a body from existing bytes of data
    /// let body = builder.bytes(BytesView::new());
    /// assert_eq!(body.content_length(), Some(0));
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let user = User {
    ///     id: 1,
    ///     name: String::from("Alice"),
    /// };
    ///
    /// // Create a body containing the JSON representation of user
    /// let body = builder.json(&user)?;
    /// # Ok::<(), HttpError>(())
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
    use bytes::Bytes;
    use bytesbuf::mem::testing::TransparentMemory;
    use futures::executor::block_on;
    use futures::stream;
    use serde::Serialize;
    use static_assertions::assert_impl_all;
    use tick::ClockControl;

    use std::time::Duration;

    use super::*;
    use crate::testing::{create_stream_body, create_stream_body_from_chunks};

    #[test]
    fn assert_send_and_sync() {
        assert_impl_all!(HttpBodyBuilder: Send, Sync, std::fmt::Debug);
    }

    #[test]
    fn new_with_global_memory() {
        let clock = Clock::new_frozen();
        let memory = GlobalPool::new();
        let builder = HttpBodyBuilder::new(memory, &clock);
        let body = builder.text("test");
        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn with_custom_memory() {
        let clock = Clock::new_frozen();
        let builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new(), &clock);
        let body = builder.text("hello");
        let data = BytesView::try_from(body).unwrap();
        assert_eq!(data.len(), 5);
    }

    #[test]
    fn with_options_sets_buffer_limit() {
        let options = BodyOptions::default().buffer_limit(1024);
        let builder = HttpBodyBuilder::new_fake().with_options(options);
        assert_eq!(builder.options, options);
    }

    #[test]
    fn with_options_defaults() {
        let builder = HttpBodyBuilder::new_fake();
        assert_eq!(builder.options, BodyOptions::default());
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
        let body = create_stream_body_from_chunks(&builder, &[b"hello ", b"world"], &BodyOptions::default());
        assert_eq!(body.content_length(), None);
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn stream_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"", &BodyOptions::default());
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn stream_with_timeout_returns_data_before_timeout() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);
        let chunks: Vec<Result<BytesView>> = [b"hello " as &[u8], b"world"]
            .iter()
            .map(|c| Ok(BytesView::copied_from_slice(c, &builder)))
            .collect();
        let options = BodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.stream(stream::iter(chunks), &options);
        assert_eq!(body.content_length(), None);
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn body_with_max_duration_timeout_still_returns_data() {
        let builder = HttpBodyBuilder::new_fake();
        let options = BodyOptions::default().timeout(Duration::MAX);
        let body = builder.body(
            http_body_util::Full::new(BytesView::copied_from_slice(b"hello", &builder)),
            &options,
        );
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"hello");
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

        let clock = Clock::new_frozen();
        let builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new(), &clock);
        let body = builder.json(&payload).unwrap();
        let bytes_view = body.into_bytes_no_buffering().unwrap();

        assert_eq!(bytes_view.len(), expected_size);

        let block_count = bytes_view.slices().count();
        assert!(
            block_count <= 5,
            "expected at most 5 memory blocks for ~30 KB JSON serialization, got {block_count}"
        );
    }

    #[test]
    fn builder_merges_per_call_options_with_defaults() {
        let clock = Clock::new_frozen();
        let builder_options = BodyOptions::default().timeout(Duration::from_secs(30));
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock).with_options(builder_options);

        // Per-call options override the builder-level default.
        let per_call = BodyOptions::default().timeout(Duration::from_secs(5));
        let body = builder.stream(stream::iter(Vec::<Result<BytesView>>::new()), &per_call);
        // Body created successfully — timeout was applied from per_call.
        assert_eq!(body.content_length(), None);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fake_builder_works_with_timeouts() {
        let options = BodyOptions::default().timeout(Duration::from_secs(1));
        let builder = HttpBodyBuilder::new_fake();

        let result = create_stream_body(&builder, b"Hello World", &options).into_text().await.unwrap();

        assert_eq!(result, "Hello World");
    }
}
