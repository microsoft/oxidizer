// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(any(feature = "test-util", test))]
use bytesbuf::mem::GlobalPool;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};
use futures::{Stream, TryStreamExt};
use http_body::{Body, Frame};
use thread_aware::ThreadAware;
use tick::Clock;

use super::options::HttpBodyOptions;
use super::{HttpBody, StreamingBody};
#[cfg(any(feature = "json", test))]
use crate::json::JsonError;
use crate::{HttpError, Result};

/// A builder for HTTP bodies.
///
/// The builder provides:
///
/// - Configurable memory through [`OpaqueMemory`].
/// - Optimizations for each runtime.
/// - Test support through the `test-util` feature.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "test-util")] {
/// # use http_extensions::{HttpBody, HttpBodyBuilder};
/// # let builder = HttpBodyBuilder::new_fake();
/// // Create different body types.
/// let text_body: HttpBody = builder.text("Hello world");
/// let empty_body: HttpBody = builder.empty();
/// let binary_body: HttpBody = builder.slice(&[1, 2, 3, 4]);
/// # }
/// # }
/// ```
///
/// # Testing
///
/// The `test-util` feature provides [`HttpBodyBuilder::new_fake`].
#[derive(Debug, Clone, ThreadAware)]
pub struct HttpBodyBuilder {
    memory: OpaqueMemory,
    clock: Clock,
    pub(super) options: HttpBodyOptions,
}

impl HttpBodyBuilder {
    /// Creates a test-friendly [`HttpBodyBuilder`] instance.
    ///
    /// Use this method in unit tests.
    /// The `test-util` feature provides it.
    /// It creates HTTP bodies without a network or server.
    ///
    /// Uses a frozen clock, so body data receive timeouts will never fire.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::HttpBodyBuilder;
    /// let builder = HttpBodyBuilder::new_fake();
    /// let text_body = builder.text("Test content");
    /// # }
    /// # }
    /// ```
    #[cfg(any(feature = "test-util", test))]
    #[must_use]
    pub fn new_fake() -> Self {
        Self::new(GlobalPool::new(), &Clock::new_frozen())
    }

    /// Creates a new instance of [`HttpBodyBuilder`].
    ///
    /// The supplied memory provider serves body allocations.
    /// Pass a [`bytesbuf::mem::GlobalPool`] directly or wrap a custom provider in [`OpaqueMemory`].
    /// Cloning the builder clones the provider.
    /// [`ThreadAware::relocate`] forwards relocation to the provider.
    #[must_use]
    pub fn new(memory: impl Into<OpaqueMemory>, clock: &Clock) -> Self {
        Self {
            memory: memory.into(),
            clock: clock.clone(),
            options: HttpBodyOptions::default(),
        }
    }

    /// Sets default [`HttpBodyOptions`] for all bodies created by this builder.
    ///
    /// Options for [`body`](Self::body) or [`stream`](Self::stream) override these defaults.
    #[must_use]
    pub fn with_options(mut self, options: HttpBodyOptions) -> Self {
        self.options = options;
        self
    }

    /// Creates an `HttpBody` from any body implementation.
    ///
    /// Use this to integrate custom types that implement [`http_body::Body`] with the
    /// [`HttpBody`] system. Useful for third-party libraries or your own custom body
    /// implementations.
    ///
    /// If the merged options contain a timeout, `HttpBody` applies it while polling.
    /// The timeout limits the wait for the next frame.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::{HttpBodyOptions, HttpBodyBuilder, HttpError, HttpBody};
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
    /// let body = builder.body(custom_body, &HttpBodyOptions::default());
    /// # }
    /// # }
    /// ```
    pub fn body<B>(&self, body: B, options: &HttpBodyOptions) -> HttpBody
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        let merged = options.merge(&self.options);
        HttpBody::from_streaming(StreamingBody::new(body), merged, self.clock.clone(), None)
    }

    /// Creates a body from a stream of byte chunks.
    ///
    /// Accepts a [`Stream`][futures::Stream] of [`BytesView`] chunks and creates a streaming
    /// body from them.
    ///
    /// If the merged options contain a timeout, `HttpBody` applies it while polling.
    /// The timeout limits the wait for the next frame.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::{HttpBodyOptions, HttpBodyBuilder, HttpError};
    /// # use bytesbuf::BytesView;
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let chunks = vec![
    ///     Ok(BytesView::copied_from_slice(b"hello ", &builder)),
    ///     Ok(BytesView::copied_from_slice(b"world", &builder)),
    /// ];
    /// let body = builder.stream(futures::stream::iter(chunks), &HttpBodyOptions::default());
    ///
    /// assert_eq!(body.content_length(), None); // unknown length for streams
    ///
    /// # }
    /// # }
    /// ```
    pub fn stream<S>(&self, stream: S, options: &HttpBodyOptions) -> HttpBody
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
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::{HttpBodyBuilder, HttpBody};
    /// #
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let body1 = builder.text("Hello, world!"); // From &str
    /// let body2 = builder.text(String::from("Hello, world!")); // From String
    ///
    /// assert_eq!(body1.content_length(), body2.content_length());
    /// # }
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
    /// This method copies the byte slice.
    /// Use [`bytes()`][Self::bytes] to avoid this copy.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::HttpBodyBuilder;
    /// #
    /// # let builder = HttpBodyBuilder::new_fake();
    /// // "Hello" in ASCII
    /// let data = [0x48, 0x65, 0x6C, 0x6C, 0x6F];
    /// let body = builder.slice(&data);
    ///
    /// assert_eq!(body.content_length(), Some(5));
    /// # }
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
    /// This method does not copy the contents of the [`BytesView`].
    /// [`slice()`][Self::slice] copies the data.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::HttpBodyBuilder;
    /// # use bytesbuf::BytesView;
    /// #
    /// # let builder = HttpBodyBuilder::new_fake();
    /// // Create a body from existing bytes of data
    /// let body = builder.bytes(BytesView::new());
    /// assert_eq!(body.content_length(), Some(0));
    /// # }
    /// # }
    /// ```
    pub fn bytes(&self, b: impl Into<BytesView>) -> HttpBody {
        HttpBody::from_bytes(b.into(), self.options, self.clock.clone())
    }

    /// Creates an empty body (zero bytes).
    ///
    /// Use this for HTTP methods that do not need a body, such as GET and HEAD.
    /// You can also use it for responses that contain only status codes or headers.
    pub fn empty(&self) -> HttpBody {
        HttpBody::from_empty(self.options, self.clock.clone())
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
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # (|| {
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
    /// # })().unwrap();
    /// # }
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

impl AsRef<Clock> for HttpBodyBuilder {
    fn as_ref(&self) -> &Clock {
        &self.clock
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytes::Bytes;
    use bytesbuf::mem::CallbackMemory;
    use bytesbuf::mem::testing::TransparentMemory;
    use futures::channel::mpsc;
    use futures::executor::block_on;
    use futures::stream;
    use http_body_util::BodyExt;
    use ohno::Labeled as _;
    use serde::Serialize;
    use static_assertions::assert_impl_all;
    use thread_aware::unaware;
    use tick::ClockControl;

    use super::*;
    use crate::transform::Content;

    #[test]
    fn transforming_keeps_the_policies_the_body_already_had() {
        let builder = HttpBodyBuilder::new_fake();
        let strict = HttpBodyOptions::default().buffer_limit(7).timeout(Duration::from_secs(3));

        // A streaming body carries its own policies through transformation.
        let body = builder.stream(futures::stream::empty(), &strict);
        let transformed = body.transform(Ok).unwrap();

        assert_eq!(transformed.options(), strict);
    }

    #[test]
    fn transforming_preserves_the_source_idle_timeout() {
        let control = ClockControl::new();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &control.to_clock())
            .with_options(HttpBodyOptions::default().timeout(Duration::from_secs(1)));
        let options = HttpBodyOptions::default().timeout(Duration::from_millis(100));
        let source = builder.stream(stream::pending(), &options);
        let mut body = pin!(source.transform(Ok).unwrap());
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(body.as_mut().poll_frame(&mut cx).is_pending());
        control.advance(Duration::from_millis(101));

        let Poll::Ready(Some(Err(error))) = body.as_mut().poll_frame(&mut cx) else {
            panic!("the source idle timeout must still fire after transformation");
        };
        assert_eq!(error.label(), "body_timeout");
    }

    #[test]
    fn transforming_times_output_frames_while_the_source_makes_progress() {
        let control = ClockControl::new();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &control.to_clock());
        let options = HttpBodyOptions::default().timeout(Duration::from_millis(100));
        let (sender, receiver) = mpsc::unbounded();
        let source = builder.stream(receiver, &options);
        let mut body = pin!(source.transform(|content| Ok(match content {
            Content::Streaming(body) => {
                Content::Streaming(StreamingBody::new(http_body_util::StreamBody::new(stream::once(async move {
                    body.collect()
                        .await
                        .map(|collected| Frame::data(BytesView::from(collected.to_bytes())))
                }))))
            }
            Content::Bytes(bytes) => Content::Bytes(bytes),
        })).unwrap());
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(body.as_mut().poll_frame(&mut cx).is_pending());
        control.advance(Duration::from_millis(75));
        sender.unbounded_send(Ok(BytesView::copied_from_slice(b"first", &builder))).unwrap();
        assert!(body.as_mut().poll_frame(&mut cx).is_pending());

        // Output is buffered for longer than the timeout, but source frames keep arriving.
        control.advance(Duration::from_millis(75));
        sender
            .unbounded_send(Ok(BytesView::copied_from_slice(b"second", &builder)))
            .unwrap();
        let Poll::Ready(Some(Err(error))) = body.as_mut().poll_frame(&mut cx) else {
            panic!("source progress must not reset the timeout for transformed frames");
        };
        assert_eq!(error.label(), "body_timeout");
    }

    #[test]
    fn body_applies_builder_defaults_to_a_returned_buffered_body() {
        let lenient = HttpBodyOptions::default().buffer_limit(99);
        let builder = HttpBodyBuilder::new_fake().with_options(lenient);

        let original = builder.text("hello").transform(Ok).unwrap();
        let transformed = builder.body(original, &HttpBodyOptions::new());

        assert_eq!(transformed.options(), lenient);
    }

    #[test]
    fn body_applies_builder_timeout_to_a_transformed_body_without_one() {
        #[derive(Debug)]
        struct PendingBody;

        impl Body for PendingBody {
            type Data = BytesView;
            type Error = HttpError;

            fn poll_frame(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Option<Result<Frame<Self::Data>>>> {
                std::task::Poll::Pending
            }
        }

        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let options = HttpBodyOptions::default().timeout(Duration::from_millis(100));
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock).with_options(options);
        let source = HttpBodyBuilder::new_fake().body(PendingBody, &HttpBodyOptions::new());
        let transformed = source.transform(Ok).unwrap();
        let mut body = Box::pin(builder.body(transformed, &HttpBodyOptions::new()));
        let waker = futures::task::noop_waker();
        let mut cx = std::task::Context::from_waker(&waker);

        assert!(body.as_mut().poll_frame(&mut cx).is_pending());

        let result = body.as_mut().poll_frame(&mut cx);
        assert!(matches!(result, std::task::Poll::Ready(Some(Err(_)))));
    }

    use crate::testing::{create_stream_body, create_stream_body_from_chunks};

    #[test]
    fn assert_send_and_sync() {
        assert_impl_all!(HttpBodyBuilder: Send, Sync, AsRef<Clock>, std::fmt::Debug);
    }

    #[test]
    fn new_with_global_memory() {
        let clock = Clock::new_frozen();
        let memory = GlobalPool::new();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(memory), &clock);
        let body = builder.text("test");
        assert_eq!(body.content_length(), Some(4));

        // access the clock
        let _clock: &Clock = builder.as_ref();
    }

    #[test]
    fn new_accepts_a_custom_provider() {
        let clock = Clock::new_frozen();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(TransparentMemory::new()), &clock);
        let body = builder.text("hello");
        let data = BytesView::try_from(body).unwrap();
        assert_eq!(data.len(), 5);
    }

    #[test]
    fn with_options_sets_buffer_limit() {
        let options = HttpBodyOptions::default().buffer_limit(1024);
        let builder = HttpBodyBuilder::new_fake().with_options(options);
        assert_eq!(builder.options, options);
    }

    #[test]
    fn with_options_defaults() {
        let builder = HttpBodyBuilder::new_fake();
        assert_eq!(builder.options, HttpBodyOptions::default());
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
        let body = create_stream_body_from_chunks(&builder, &[b"hello ", b"world"], &HttpBodyOptions::default());
        assert_eq!(body.content_length(), None);
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn stream_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"", &HttpBodyOptions::default());
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn stream_with_timeout_returns_data_before_timeout() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);
        let chunks: Vec<Result<BytesView>> = [b"hello " as &[u8], b"world"]
            .iter()
            .map(|c| Ok(BytesView::copied_from_slice(c, &builder)))
            .collect();
        let options = HttpBodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.stream(stream::iter(chunks), &options);
        assert_eq!(body.content_length(), None);
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn body_with_max_duration_timeout_still_returns_data() {
        let builder = HttpBodyBuilder::new_fake();
        let options = HttpBodyOptions::default().timeout(Duration::MAX);
        let body = builder.body(
            http_body_util::Full::new(BytesView::copied_from_slice(b"hello", &builder)),
            &options,
        );
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"hello");
    }

    // ── JSON body creation ───────────────────────────────────────────────

    /// Observes JSON allocations through the selected provider in ordinary test builds.
    #[test]
    fn json_uses_the_supplied_memory_provider() {
        let reservations = Arc::new(Mutex::new(Vec::new()));
        let memory = CallbackMemory::new(unaware(Arc::clone(&reservations)), |reservations, min_bytes| {
            reservations.lock().unwrap().push(min_bytes);
            TransparentMemory::new().reserve(min_bytes)
        });
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(memory), &Clock::new_frozen());

        let body = builder.json(&["caller", "memory"]).unwrap();

        assert_eq!(BytesView::try_from(body).unwrap(), br#"["caller","memory"]"#);
        assert!(!reservations.lock().unwrap().is_empty());
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "allocation-count performance coverage is native-only; Miri retains body serialization with bounded payloads"
    )]
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
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(TransparentMemory::new()), &clock);
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
        let builder_options = HttpBodyOptions::default().timeout(Duration::from_secs(30));
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock).with_options(builder_options);

        // Per-call options override the builder-level default.
        let per_call = HttpBodyOptions::default().timeout(Duration::from_secs(5));
        let body = builder.stream(stream::iter(Vec::<Result<BytesView>>::new()), &per_call);
        // The body uses the timeout from per_call.
        assert_eq!(body.content_length(), None);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fake_builder_works_with_timeouts() {
        let options = HttpBodyOptions::default().timeout(Duration::from_secs(1));
        let builder = HttpBodyBuilder::new_fake();

        let result = create_stream_body(&builder, b"Hello World", &options).into_text().await.unwrap();

        assert_eq!(result, "Hello World");
    }
}
