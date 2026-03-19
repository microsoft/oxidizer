// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP body types for requests and responses.
//!
//! This module provides a flexible way to handle HTTP bodies. The main type is [`HttpBody`],
//! which can work with:
//!
//! - Text and string data
//! - Binary data
//! - JSON data (with the `json` feature)
//! - Streaming content
//! - Empty bodies
//! - Custom body implementations
//!
//! Bodies are created through [`HttpBodyBuilder`], which optimizes memory usage and supports testing.
//! All bodies implement the standard `http_body::Body` trait for ecosystem compatibility.

use std::fmt::{Debug, Formatter};
use std::io::Read;
use std::pin::Pin;
use std::task::Poll::Ready;
use std::task::{Context, Poll};

use bytesbuf::mem::{GlobalPool, HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};
use futures::{Stream, TryStreamExt};
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
#[cfg(any(feature = "hyper", test))]
use hyper::body::Incoming;
use pin_project::pin_project;
use thread_aware::{ThreadAware, Unaware};

use crate::constants::DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES;
#[cfg(any(feature = "json", test))]
use crate::json::JsonError;
use crate::{HttpError, Result};

/// A flexible HTTP body container for various data types.
///
/// `HttpBody` handles text, binary data, JSON (with the `json` feature), streaming content,
/// and more. Always use the [`HttpBodyBuilder`] to create bodies instead of constructing
/// them directly.
///
/// # Examples
///
/// ```
/// # use http_extensions::HttpBodyBuilder;
/// # async fn example(builder: &HttpBodyBuilder) {
/// // Create different body types
/// let text_body = builder.text("Hello world");
/// let binary_body = builder.slice(&[1, 2, 3, 4]);
/// let empty_body = builder.empty();
/// # }
/// ```
///
/// # How does `HttpBody` work?
///
/// At its core, [`HttpBody`] implements the [`http_body::Body`] trait, which provides a standardized
/// way to process HTTP data as it arrives. Here's what makes it efficient:
///
/// 1. **Streaming data**: Instead of waiting for the entire response, data comes in as individual
///    frames via [`http_body::Frame`] that you can process immediately.
/// 2. **Memory efficiency**: Each chunk is represented by a [`bytesbuf::BytesView`], ensuring that
///    its backing memory gets automatically returned to a memory pool when consumed.
/// 3. **Zero-copy when possible**: The implementation uses [`bytesbuf::BytesView`] under the hood and
///    avoids unnecessary copying, making it efficient for large responses.
///
/// When you call [`HttpBody::poll_frame`], the body notifies you when new data is available through
/// a [`Poll`] result. After consuming data from a [`BytesView`], its memory is automatically
/// recycled back to the memory pool, reducing allocation overhead for future requests.
///
/// # Buffering
///
/// When you call [`HttpBody::into_buffered`], the entire body content is loaded into memory
/// and the underlying network connection is freed. This is helpful when you need to:
///
/// - Process the same data multiple times
/// - Clone a streaming body that would otherwise be consumed
/// - Keep data available after the network connection would close
///
/// Buffering has a configurable memory limit (default: 2 GB) to prevent out-of-memory issues
/// with extremely large responses. If a response exceeds this limit, you'll get an error.
/// You can customize this limit via [`HttpBodyBuilder::with_response_buffer_limit`].
///
/// # Streaming
///
/// Process bodies in a streaming fashion to handle data chunk-by-chunk as it arrives.
///
/// Streaming is great for processing data incrementally. This is useful when you know that you are
/// working with large HTTP response bodies, e.g., downloading large files. Unlike regular responses,
/// **streaming has no buffering limits**, so you can efficiently handle bodies of any size without
/// worrying about memory constraints.
///
/// The example below shows how to download a large file and write it to disk:
///
/// ```
/// use futures::TryStreamExt; // For stream operations
/// use http_body_util::BodyExt; // For into_data_stream() method
/// use http_extensions::{HttpBody, HttpError};
///
/// async fn download_to_file(
///     body: HttpBody,
///     output_file: &mut std::fs::File,
/// ) -> Result<(), HttpError> {
///     // Convert the response into a stream
///     let mut stream = body.into_data_stream();
///
///     // Process each chunk as it arrives
///     while let Some(mut data) = stream.try_next().await? {
///         // Write the chunk to the output file (BytesView implements Read)
///         std::io::copy(&mut data, output_file)?;
///     }
///
///     Ok(())
/// }
/// ```
///
/// To simplify processing, you convert the `HttpBody` into a [`Stream`][futures::Stream] by calling
/// the [`into_data_stream`][http_body_util::BodyExt::into_data_stream] method. Then you can use the
/// many extensions methods provided for the [`Stream`][futures::Stream] trait to process the data.
///
/// > **Note**: The above example uses the [`futures`] crate for stream processing and the [`http_body_util`] crate
/// > for converting the body into a data stream.
///
/// [http-body-util]: https://docs.rs/http-body-util/latest/http_body_util/
/// [futures]: https://docs.rs/futures/latest/futures/
#[derive(Debug, ThreadAware)]
#[pin_project]
#[must_use]
pub struct HttpBody {
    #[pin]
    #[thread_aware(skip)]
    kind: Kind,
    builder: HttpBodyBuilder,
}

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
/// With the `test-util` feature enabled, you can create a test instance using `HttpBodyBuilder::fake()`.
#[derive(Debug, Clone, ThreadAware)]
pub struct HttpBodyBuilder {
    memory: MemoryWrapper,
    response_buffer_limit: Option<usize>,
}

// Implementations for HttpBody

impl HttpBody {
    const fn new(kind: Kind, builder: HttpBodyBuilder) -> Self {
        Self { kind, builder }
    }

    /// Converts the body into a memory-efficient view over a byte sequence.
    ///
    /// Useful when you need direct access to the raw bytes without extra conversions.
    ///
    /// # Errors
    ///
    /// Returns an error if the body can't be collected or was already consumed.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBody, HttpError};
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     let body_bytes = body.into_bytes().await?;
    ///     println!("Received {} bytes", body_bytes.len());
    ///     Ok(())
    /// }
    /// ```
    pub async fn into_bytes(self) -> Result<BytesView> {
        self.into_buffered()
            .await?
            .into_bytes_no_buffering()
            .map_or_else(|| unreachable!("once body is buffered, it must be a view over a byte sequence"), Ok)
    }

    pub(crate) fn into_bytes_no_buffering(self) -> Option<BytesView> {
        match self.kind {
            Kind::Bytes(Some(bytes)) => Some(bytes),
            Kind::Empty => Some(BytesView::default()),
            _ => None,
        }
    }

    /// Consumes the body and converts it to a UTF-8 string.
    ///
    /// Useful for text-based responses like HTML or plain text.
    ///
    /// # Errors
    ///
    /// Returns an error if a collection fails or if the content contains invalid UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBody, HttpError};
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     let text = body.into_text().await?;
    ///     println!("Received: {}", text);
    ///     Ok(())
    /// }
    /// ```
    #[expect(clippy::cast_possible_truncation, reason = "size_hint is used for capacity, not exact size")]
    pub async fn into_text(self) -> Result<String> {
        let mut text = String::with_capacity(self.size_hint().lower() as usize);

        self.into_bytes()
            .await?
            .read_to_string(&mut text)
            .map_err(|e| HttpError::validation(format!("body contains invalid UTF-8: {e}")))?;

        Ok(text)
    }

    /// Loads the entire body into memory for easier handling.
    ///
    /// Useful when you need to:
    ///
    /// - Read a streaming body multiple times
    /// - Clone a body that couldn't be cloned before
    /// - Preload data to avoid connection issues later
    ///
    /// To change the memory limit, use [`HttpBodyBuilder::with_response_buffer_limit`].
    /// By default, it's capped at `2GB` to prevent memory issues.
    ///
    /// # Caveats
    ///
    /// Be careful with large bodies; this loads everything into memory at once.
    ///
    /// # Errors
    ///
    /// - Network problems while collecting data
    /// - Body already consumed elsewhere
    ///
    /// # Examples
    ///
    /// ```
    /// use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     // Load the entire body into memory
    ///     let buffered = body.into_buffered().await?;
    ///
    ///     // Now you can work with it multiple times...
    ///     Ok(())
    /// }
    /// ```
    pub async fn into_buffered(self) -> Result<Self> {
        let builder = self.builder;
        let limit = builder.response_buffer_limit;

        match self.kind {
            #[cfg(any(feature = "hyper", test))]
            Kind::Incoming(incoming) => {
                let data = collect_with_limit(map_incoming_stream(incoming), limit).await?;
                Ok(builder.bytes(data))
            }
            Kind::Bytes(Some(data)) => Ok(builder.bytes(data)),
            Kind::Bytes(None) => Err(HttpError::validation("body cannot be buffered because it is already consumed")),
            Kind::Empty => Ok(builder.empty()),
            Kind::Body(b) => {
                let data = collect_with_limit(b.into_data_stream(), limit).await?;
                Ok(builder.bytes(data))
            }
        }
    }

    /// Consumes the body and converts it to JSON.
    ///
    /// Converts the body directly to your desired type. Available with the `json` feature.
    ///
    /// # Errors
    ///
    /// Returns an error if the body can't be collected or JSON parsing fails.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpBody};
    /// # use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct User {
    ///     id: u32,
    ///     name: String,
    ///     is_active: bool,
    /// }
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     // Parse the JSON body into a structured type
    ///     let user: User = body.into_json_owned().await?;
    ///
    ///     println!("Received user: {} (ID: {})", user.name, user.id);
    ///
    ///     // Process the user data...
    ///     Ok(())
    /// }
    /// ```
    #[cfg(any(feature = "json", test))]
    pub async fn into_json_owned<T: serde_core::de::DeserializeOwned>(self) -> Result<T> {
        let json = self.into_json().await?.read_owned()?;
        Ok(json)
    }

    /// Consumes the body and converts it to a zero-copy JSON parser.
    ///
    /// This method provides zero-copy JSON parsing by working directly with the underlying
    /// memory buffer. Unlike [`into_json_owned`][HttpBody::into_json_owned], this method can work with borrowed data
    /// and types that use `Cow<str>` for efficient string handling.
    ///
    /// The returned [`Json<T>`][crate::Json] allows you to deserialize into types that can borrow from
    /// the original data, making it more memory-efficient for large JSON payloads.
    ///
    /// # Errors
    ///
    /// Returns an error if the body can't be collected into a sequence.
    /// JSON parsing errors occur when you call methods on the returned [`Json<T>`][crate::Json].
    ///
    /// # Examples
    ///
    /// Basic usage with borrowed strings:
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpError, HttpBody};
    /// # use serde::Deserialize;
    /// # use std::borrow::Cow;
    ///
    /// #[derive(Deserialize)]
    /// struct User<'a> {
    ///     id: u32,
    ///     #[serde(borrow)]
    ///     name: Cow<'a, str>,
    ///     #[serde(borrow)]
    ///     email: Cow<'a, str>,
    ///     is_active: bool,
    /// }
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     // Parse JSON while potentially borrowing string data
    ///     let mut json = body.into_json::<User>().await?;
    ///     let user = json.read()?;
    ///
    ///     println!("User: {} <{}> (ID: {})", user.name, user.email, user.id);
    ///
    ///     // The name and email fields may be borrowed from the original JSON buffer,
    ///     // avoiding unnecessary string allocations for better performance
    ///     Ok(())
    /// }
    /// ```
    ///
    /// For types that don't need borrowing, prefer [`into_json_owned`][HttpBody::into_json_owned] for simpler usage.
    #[cfg(any(feature = "json", test))]
    pub async fn into_json<'a, T: serde_core::de::Deserialize<'a>>(self) -> Result<crate::json::Json<T>> {
        Ok(crate::json::Json::<T>::new(self.into_bytes().await?))
    }

    /// Gets the body's content length in bytes, if known.
    ///
    /// Checks the size without consuming the body - used for setting `Content-Length`
    /// headers or pre-allocating buffers.
    ///
    /// Returns `Some(size)` for known-length bodies, `None` for streaming bodies with unknown length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::{HttpBodyBuilder, HttpBody};
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// let text_body = create_body.text("Hello, world!");
    /// assert_eq!(text_body.content_length(), Some(13));
    ///
    /// let empty_body = create_body.empty();
    /// assert_eq!(empty_body.content_length(), Some(0));
    /// # }
    /// ```
    #[must_use]
    pub fn content_length(&self) -> Option<u64> {
        match &self.kind {
            #[cfg(any(feature = "hyper", test))]
            Kind::Incoming(incoming) => incoming.size_hint().exact(),
            Kind::Bytes(Some(bytes)) => Some(bytes.len() as u64),
            Kind::Bytes(None) | Kind::Empty => Some(0),
            Kind::Body(b) => b.size_hint().exact(),
        }
    }

    /// Returns `true` if the body is known to be empty (zero bytes).
    ///
    /// This checks whether the body's content length is exactly zero.
    /// For streaming bodies with unknown length, this returns `false`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use http_extensions::HttpBodyBuilder;
    /// # fn example(create_body: &HttpBodyBuilder) {
    /// let empty_body = create_body.empty();
    /// assert!(empty_body.is_empty());
    ///
    /// let text_body = create_body.text("Hello");
    /// assert!(!text_body.is_empty());
    /// # }
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.content_length() == Some(0)
    }

    /// Attempts to clone the body if possible.
    ///
    /// This is only supported for bodies created from static data like text or byte slices.
    /// Streaming bodies or those created from custom implementations cannot be cloned.
    ///
    /// To turn a non-cloneable body into a cloneable one, use [`into_buffered()`][HttpBody::into_buffered()]
    /// to load the entire content into memory first. Note that this may have memory implications
    /// for large bodies.
    #[must_use]
    pub fn try_clone(&self) -> Option<Self> {
        match &self.kind {
            Kind::Bytes(Some(bytes)) => Some(self.builder.bytes(bytes.clone())),
            Kind::Empty => Some(self.builder.empty()),
            #[cfg(any(feature = "hyper", test))]
            Kind::Incoming(_) => None,
            Kind::Body(_) | Kind::Bytes(None) => None,
        }
    }

    /// Converts this body into a stream of byte chunks.
    ///
    /// This is a convenience wrapper around
    /// [`BodyExt::into_data_stream()`][http_body_util::BodyExt::into_data_stream],
    /// eliminating the need to import `http_body_util::BodyExt` directly.
    ///
    /// # Examples
    ///
    /// ```
    /// use futures::TryStreamExt;
    /// use http_extensions::{HttpBody, HttpError};
    ///
    /// async fn process_body(body: HttpBody) -> Result<(), HttpError> {
    ///     let mut stream = body.into_stream();
    ///
    ///     while let Some(chunk) = stream.try_next().await? {
    ///         println!("received {} bytes", chunk.len());
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn into_stream(self) -> impl Stream<Item = Result<BytesView>> {
        self.into_data_stream()
    }
}

impl TryFrom<HttpBody> for BytesView {
    type Error = HttpError;

    fn try_from(value: HttpBody) -> std::result::Result<Self, Self::Error> {
        match value.kind {
            Kind::Bytes(Some(bytes)) => Ok(bytes),
            Kind::Empty => Ok(Self::default()),
            _ => Err(HttpError::validation(
                "body cannot be converted to byte sequence because it is not buffered",
            )),
        }
    }
}

/// Makes `HttpBody` compatible with the standard HTTP ecosystem.
///
/// By implementing `http_body::Body`, our `HttpBody` type works seamlessly with any HTTP
/// client or server that uses this standard interface.
impl Body for HttpBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        let this = self.project();

        match this.kind.project() {
            #[cfg(any(feature = "hyper", test))]
            BodyInnerProj::Incoming(inner) => match inner.poll_frame(cx) {
                Ready(Some(res)) => Ready(Some(match res {
                    Ok(frame) => Ok(frame.map_data(Into::into)),
                    Err(e) => Err(HttpError::other(e, recoverable::RecoveryInfo::unknown(), "hyper")),
                })),
                Ready(None) => Ready(None),
                Poll::Pending => Poll::Pending,
            },
            BodyInnerProj::Bytes(bytes) => bytes
                .take()
                .map_or_else(|| Ready(None), |bytes| Ready((!bytes.is_empty()).then(|| Ok(Frame::data(bytes))))),
            BodyInnerProj::Empty => Ready(None),
            BodyInnerProj::Body(body) => body.as_mut().poll_frame(cx),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            #[cfg(any(feature = "hyper", test))]
            Kind::Incoming(incoming) => incoming.size_hint(),
            Kind::Bytes(Some(bytes)) => SizeHint::with_exact(bytes.len() as u64),
            Kind::Bytes(None) | Kind::Empty => SizeHint::with_exact(0),
            Kind::Body(b) => b.size_hint(),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            #[cfg(any(feature = "hyper", test))]
            Kind::Incoming(incoming) => incoming.is_end_stream(),
            Kind::Bytes(Some(x)) => x.is_empty(),
            Kind::Bytes(None) | Kind::Empty => true,
            Kind::Body(b) => b.is_end_stream(),
        }
    }
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

    /// Creates an `HttpBody` from a Hyper [`Incoming`] body.
    #[cfg(any(feature = "hyper", test))]
    pub fn incoming(&self, inner: Incoming) -> HttpBody {
        HttpBody::new(Kind::Incoming(inner), self.clone())
    }

    /// Creates an `HttpBody` from any custom body implementation.
    ///
    /// Use this to integrate custom types that implement `http_body::Body` with the `HttpBody` system.
    /// Useful for third-party libraries or your own custom body implementations.
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
    /// let body = create_body.external(custom_body);
    /// # }
    /// ```
    pub fn external<B>(&self, body: B) -> HttpBody
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        HttpBody::new(Kind::Body(Box::pin(body.map_err(Into::into))), self.clone())
    }

    /// Creates a body from a stream of byte chunks.
    ///
    /// This is a convenience wrapper around [`external`][Self::external] that accepts
    /// a [`Stream`][futures::Stream] of [`BytesView`] chunks. It avoids the need to
    /// manually wrap the stream in a [`StreamBody`][http_body_util::StreamBody].
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
        self.external(StreamBody::new(framed))
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

#[expect(
    clippy::large_enum_variant,
    reason = "BytesView is intentionally large, though future optimizations may decrease size"
)]
#[pin_project(project = BodyInnerProj)]
enum Kind {
    #[cfg(any(feature = "hyper", test))]
    Incoming(#[pin] Incoming),
    Bytes(Option<BytesView>),
    Empty,
    Body(Pin<Box<dyn Body<Data = BytesView, Error = HttpError> + Send>>),
}

impl Debug for Kind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(any(feature = "hyper", test))]
            Self::Incoming(_) => f.debug_struct("Incoming").finish(),
            Self::Bytes(_) => f.debug_struct("Bytes").finish(),
            Self::Empty => f.debug_struct("Empty").finish(),
            Self::Body(_) => f.debug_struct("Body").finish(),
        }
    }
}

#[cfg(any(feature = "hyper", test))]
fn map_incoming_stream(incoming: Incoming) -> impl Stream<Item = Result<BytesView>> {
    incoming
        .into_data_stream()
        .map_err(|e| HttpError::other(e, recoverable::RecoveryInfo::unknown(), "hyper"))
        .map_ok(Into::into)
}

async fn collect_with_limit(mut data: impl Stream<Item = Result<BytesView>> + Send + Unpin, limit: Option<usize>) -> Result<BytesView> {
    let mut total_size = 0_usize;
    let mut fragments = Vec::new();
    let limit = limit.unwrap_or(DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES);

    while let Some(bytes) = data.try_next().await? {
        let bytes_len = bytes.len();
        total_size = match total_size.checked_add(bytes_len) {
            Some(sum) => sum,
            None => {
                return Err(HttpError::validation(format!(
                    "body size exceeds the limit of {limit} bytes"
                )));
            }
        };

        if total_size > limit {
            return Err(HttpError::validation(format!("body size exceeds the limit of {limit} bytes")));
        }

        fragments.push(bytes);
    }

    Ok(BytesView::from_views(fragments))
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
    use std::pin::pin;
    use std::task::Waker;

    use bytes::Bytes;
    use bytesbuf::mem::testing::TransparentMemory;
    use futures::executor::block_on;
    use http_body_util::StreamBody;
    use ohno::ErrorExt;
    use serde::{Deserialize, Serialize};
    use static_assertions::assert_impl_all;

    use super::*;

    // Model for JSON serialization/deserialization tests
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Model {
        id: u32,
        name: String,
    }

    #[test]
    fn assert_send_and_sync() {
        assert_impl_all!(super::HttpBody: Send, Debug, ThreadAware);
    }

    #[test]
    fn assert_createhttpbody_is_send_and_sync() {
        assert_impl_all!(super::HttpBodyBuilder: Send, Sync, Debug);
    }

    #[test]
    fn from_and_into_json_ok() {
        let data = Model {
            id: 1,
            name: "name".to_string(),
        };
        let body = HttpBodyBuilder::new_fake().json(&data).unwrap();

        assert_eq!(Some(22), body.content_length());

        let result: Model = block_on(body.into_json_owned()).unwrap();

        assert_eq!(data, result);
    }

    #[test]
    fn json_deserialization_error() {
        // Create a body with invalid JSON content
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("{invalid json}");

        // Attempt to deserialize to our model, which should fail
        let result: Result<Model> = block_on(body.into_json_owned());
        result.unwrap_err();
    }

    #[test]
    fn into_json_with_cow_strings() {
        use std::borrow::Cow;

        #[derive(Debug, Deserialize, PartialEq)]
        struct User<'a> {
            id: u32,
            #[serde(borrow)]
            name: Cow<'a, str>,
            #[serde(borrow)]
            email: Cow<'a, str>,
            is_active: bool,
        }

        let json_data = r#"{"id": 42, "name": "Alice Smith", "email": "alice@example.com", "is_active": true}"#;
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text(json_data);

        let mut json_result = block_on(body.into_json::<User>()).unwrap();
        let user = json_result.read().unwrap();

        assert_eq!(user.id, 42);
        assert_eq!(user.name, "Alice Smith");
        assert_eq!(user.email, "alice@example.com");
        assert!(user.is_active);
        assert!(matches!(user.name, Cow::Borrowed(_)));
        assert!(matches!(user.email, Cow::Borrowed(_)));
    }

    #[test]
    fn try_clone_text_body() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("hello");

        let cloned = body.try_clone().unwrap();

        assert_eq!(block_on(body.into_text()).unwrap(), block_on(cloned.into_text()).unwrap());
    }

    #[test]
    fn try_clone_empty_body() {
        let builder = HttpBodyBuilder::new_fake();
        let empty = builder.empty();

        let cloned_empty = empty.try_clone().unwrap();

        assert_eq!(
            block_on(empty.into_bytes()).unwrap().len(),
            block_on(cloned_empty.into_bytes()).unwrap().len()
        );
    }

    #[test]
    fn custom_body_is_not_cloneable() {
        let body = HttpBodyBuilder::new_fake().external(http_body_util::Empty::new());

        assert!(body.try_clone().is_none());

        let body = block_on(body.into_buffered()).unwrap();
        assert_eq!(Some(0), body.content_length());
    }

    #[test]
    fn body_error_propagation() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.external(StreamBody::new(futures::stream::once(async {
            Err(HttpError::validation("test error"))
        })));

        let error = block_on(body.into_buffered()).unwrap_err();
        assert_eq!(error.message(), "test error");
    }

    #[test]
    fn collect_with_limit_success() {
        let memory = GlobalPool::new();
        let data = BytesView::copied_from_slice(b"test data", &memory);
        let stream = futures::stream::iter(vec![Ok(data)]);

        let result = block_on(collect_with_limit(stream, Some(100))).unwrap();
        assert_eq!(result, b"test data");
    }

    #[test]
    fn collect_with_limit_exceeds() {
        let memory = GlobalPool::new();
        let data1 = BytesView::copied_from_slice(b"test data 1", &memory);
        let data2 = BytesView::copied_from_slice(b"test data 2", &memory);
        let stream = futures::stream::iter(vec![Ok(data1), Ok(data2)]);

        let result = block_on(collect_with_limit(stream, Some(5)));

        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("body size exceeds the limit"));
    }

    #[test]
    fn collect_with_limit_stream_error() {
        let memory = GlobalPool::new();

        let error_stream = futures::stream::iter(vec![
            Ok(BytesView::copied_from_slice(b"valid data", &memory)),
            Err(HttpError::validation("stream error")),
        ]);

        let result = block_on(collect_with_limit(error_stream, Some(1000)));

        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("stream error"));
    }

    #[test]
    fn createhttpbody_with_custom_memory_provider() {
        let memory = GlobalPool::new();

        let builder = HttpBodyBuilder::new(memory);

        let body = builder.text("test");
        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn slice_creation() {
        let builder = HttpBodyBuilder::new_fake();
        let data = [1, 2, 3, 4];
        let body = builder.slice(data);

        assert_eq!(body.content_length(), Some(4));
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, &[1, 2, 3, 4]);
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
    fn collect_with_custom_limit() {
        let memory = GlobalPool::new();
        let data = BytesView::copied_from_slice(&[0u8; 1000], &memory);
        let stream = futures::stream::iter(vec![Ok(data)]);

        let result = block_on(collect_with_limit(stream, Some(1024))).unwrap();
        assert_eq!(result.len(), 1000);
    }

    #[test]
    fn into_text_invalid_utf8() {
        let builder = HttpBodyBuilder::new_fake();
        let invalid_utf8 = vec![0xFF, 0xFE, 0xFD];
        let body = builder.slice(&invalid_utf8);

        let error = block_on(body.into_text()).unwrap_err();
        assert!(error.to_string().contains("body contains invalid UTF-8"));
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
    fn content_length_empty_body() {
        let builder = HttpBodyBuilder::new_fake();
        let empty = builder.empty();
        assert_eq!(empty.content_length(), Some(0));
    }

    #[test]
    fn content_length_text_body() {
        let builder = HttpBodyBuilder::new_fake();
        let text = builder.text("hello");
        assert_eq!(text.content_length(), Some(5));
    }

    #[test]
    fn content_length_slice_body() {
        let builder = HttpBodyBuilder::new_fake();
        let slice = builder.slice([1, 2, 3]);
        assert_eq!(slice.content_length(), Some(3));
    }

    #[test]
    fn content_length_bytes_body() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.bytes(BytesView::new());
        assert_eq!(body.content_length(), Some(0));
    }

    #[test]
    fn collect_with_limit_default() {
        let memory = GlobalPool::new();
        let data = BytesView::copied_from_slice(b"test", &memory);
        let stream = futures::stream::iter(vec![Ok(data)]);

        let result = block_on(collect_with_limit(stream, None));
        result.unwrap();
    }

    #[test]
    fn collect_with_limit_empty_stream() {
        let stream = futures::stream::iter(vec![] as Vec<Result<BytesView>>);

        let result = block_on(collect_with_limit(stream, Some(100))).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn try_clone_custom_body_fails() {
        let builder = HttpBodyBuilder::new_fake();
        let custom_body = builder.external(http_body_util::Empty::new());

        assert!(custom_body.try_clone().is_none());
    }

    #[test]
    fn into_bytes_view_empty_body_ok() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.empty();

        let data = BytesView::try_from(body).unwrap();

        assert_eq!(data.len(), 0);
    }

    #[test]
    fn into_bytes_view_has_data_ok() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("hello");

        let data = BytesView::try_from(body).unwrap();

        assert_eq!(data.len(), 5);
    }

    #[test]
    fn into_bytes_view_has_data_with_custom_memory() {
        let builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new());
        let body = builder.text("hello");

        let data = BytesView::try_from(body).unwrap();

        assert_eq!(data.len(), 5);
    }

    #[test]
    fn into_bytes_custom_body_fails() {
        let builder = HttpBodyBuilder::new_fake();
        let custom_body = builder.external(http_body_util::Empty::new());

        BytesView::try_from(custom_body).unwrap_err();
    }

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
    fn bytes_view() {
        let memory = GlobalPool::new();
        let builder = HttpBodyBuilder::new_fake();
        let bytes = BytesView::copied_from_slice(b"test", &memory);
        let body = builder.bytes(bytes);

        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn into_bytes_view() {
        let builder = HttpBodyBuilder::new_fake();
        // `Bytes` can .into() `BytesView`
        let body = builder.bytes(Bytes::from_static(b"test"));

        assert_eq!(body.content_length(), Some(4));
    }

    #[test]
    fn size_hint_bytes_view() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("test");

        let size_hint = body.size_hint();
        assert_eq!(size_hint.lower(), 4);
        assert_eq!(size_hint.upper(), Some(4));
    }

    #[test]
    fn size_hint_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.empty();

        let size_hint = body.size_hint();
        assert_eq!(size_hint.lower(), 0);
        assert_eq!(size_hint.upper(), Some(0));
    }

    #[test]
    fn collect_with_limit_at_boundary() {
        let memory = GlobalPool::new();
        let data = BytesView::copied_from_slice(&[0u8; 100], &memory);
        let stream = futures::stream::iter(vec![Ok(data)]);

        let result = block_on(collect_with_limit(stream, Some(100)));
        result.unwrap();
    }

    #[test]
    fn collect_with_limit_exceeds_boundary() {
        let memory = GlobalPool::new();
        let data = BytesView::copied_from_slice(&[0u8; 101], &memory);
        let stream = futures::stream::iter(vec![Ok(data)]);

        let result = block_on(collect_with_limit(stream, Some(100)));
        result.unwrap_err();
    }

    #[test]
    fn debug_kind() {
        let debug_str = format!("{:?}", Kind::Bytes(Some(BytesView::default())));
        assert_eq!("Bytes", debug_str);

        let debug_str = format!("{:?}", Kind::Empty);
        assert_eq!("Empty", debug_str);

        let debug_str = format!("{:?}", Kind::Empty);
        assert_eq!("Empty", debug_str);

        let builder = HttpBodyBuilder::new_fake();
        let stream = futures::stream::iter(Vec::<Result<BytesView>>::new());
        let body = builder.stream(stream);
        let debug_str = format!("{body:?}");
        assert!(debug_str.contains("Body"), "{debug_str}");
    }

    #[test]
    fn http_body_poll_frame() {
        let mut http_body = pin!(HttpBodyBuilder::new_fake().text("test body"));
        let mut cx = Context::from_waker(Waker::noop());

        let res = http_body.as_mut().poll_frame(&mut cx);
        assert!(matches!(res, Poll::Ready(Some(Ok(_)))));

        let res = http_body.as_mut().poll_frame(&mut cx);
        assert!(matches!(res, Poll::Ready(None)));
    }

    #[test]
    fn poll_frame_empty_bytes_view_returns_none() {
        let builder = HttpBodyBuilder::new_fake();
        let zero_bytes = BytesView::new();
        let mut body = pin!(builder.bytes(zero_bytes));
        let mut cx = Context::from_waker(Waker::noop());

        let result = body.as_mut().poll_frame(&mut cx);
        assert!(matches!(result, Poll::Ready(None)));
    }

    #[test]
    fn is_end_stream_bytes_view_non_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("non-empty");

        assert!(!body.is_end_stream());
    }

    #[test]
    fn is_end_stream_bytes_view_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let zero_bytes = BytesView::new();
        let body = builder.bytes(zero_bytes);

        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_bytes_view_none() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("test");

        // Consume the contents to make it None
        let mut pinned = pin!(body);
        let mut cx = Context::from_waker(Waker::noop());
        let _ = pinned.as_mut().poll_frame(&mut cx);

        assert!(pinned.is_end_stream());
    }

    #[test]
    fn is_end_stream_empty_body() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.empty();

        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_custom_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let custom = http_body_util::Empty::new();
        let body = builder.external(custom);

        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_custom_body_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let data = Bytes::from_static(b"test data");
        let custom = http_body_util::Full::new(data.into());
        let body = builder.external(custom);

        assert!(!body.is_end_stream());
    }

    #[test]
    fn json_serialization_makes_few_memory_allocations() {
        // A model with enough data to produce ~30 KB of JSON.
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

        // Sanity-check: the JSON should be roughly 30 KB.
        let expected_size = serde_json::to_vec(&payload).unwrap().len();
        assert!(
            expected_size > 25_000 && expected_size < 40_000,
            "expected ~30 KB JSON, got {expected_size} bytes"
        );

        let builder = HttpBodyBuilder::with_custom_memory(TransparentMemory::new());
        let body = builder.json(&payload).unwrap();

        let bytes_view = body.into_bytes_no_buffering().unwrap();

        assert_eq!(bytes_view.len(), expected_size);

        // Count the number of separate memory blocks that were allocated.
        // With TransparentMemory, each reserve() call creates a new block, so the block count
        // directly reflects the number of memory allocations performed during serialization.
        let block_count = bytes_view.slices().count();

        // With efficient buffering we expect very few allocations (ideally 1).
        // Allow up to 5 as a permissive threshold.
        assert!(
            block_count <= 5,
            "expected at most 5 memory blocks for ~30 KB JSON serialization, got {block_count}"
        );
    }

    // ── Incoming (real hyper body) tests ─────────────────────────────────

    #[tokio::test]
    async fn incoming_into_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"raw bytes").await;
        let body = builder.incoming(incoming);
        let bytes = body.into_bytes().await.unwrap();
        assert_eq!(bytes, b"raw bytes");
    }

    #[tokio::test]
    async fn incoming_empty_into_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"").await;
        let body = builder.incoming(incoming);
        let bytes = body.into_bytes().await.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn incoming_into_json_owned() {
        let builder = HttpBodyBuilder::new_fake();
        let json_bytes = br#"{"id":42,"name":"alice"}"#;
        let incoming = crate::testing::create_incoming(json_bytes).await;
        let body = builder.incoming(incoming);
        let model: Model = body.into_json_owned().await.unwrap();
        assert_eq!(
            model,
            Model {
                id: 42,
                name: "alice".to_string()
            }
        );
    }

    #[tokio::test]
    async fn incoming_into_json_zero_copy() {
        use std::borrow::Cow;

        #[derive(Debug, Deserialize, PartialEq)]
        struct Msg<'a> {
            #[serde(borrow)]
            text: Cow<'a, str>,
        }

        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(br#"{"text":"hello"}"#).await;
        let body = builder.incoming(incoming);
        let mut json = body.into_json::<Msg>().await.unwrap();
        let msg = json.read().unwrap();
        assert_eq!(msg.text, "hello");
    }

    #[tokio::test]
    async fn incoming_try_clone_returns_none() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"no clone").await;
        let body = builder.incoming(incoming);
        assert!(body.try_clone().is_none());
    }

    #[tokio::test]
    async fn incoming_into_bytes_view_fails() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"not buffered").await;
        let body = builder.incoming(incoming);
        BytesView::try_from(body).unwrap_err();
    }

    #[tokio::test]
    async fn incoming_into_bytes_no_buffering_returns_none() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"data").await;
        let body = builder.incoming(incoming);
        assert!(body.into_bytes_no_buffering().is_none());
    }

    #[tokio::test]
    async fn incoming_size_hint() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"twelve bytes").await;
        let body = builder.incoming(incoming);
        let hint = body.size_hint();
        // Incoming bodies from hyper report a size hint based on Content-Length.
        assert_eq!(hint.lower(), 12);
    }

    #[tokio::test]
    async fn incoming_content_length() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"len").await;
        let body = builder.incoming(incoming);
        assert_eq!(body.content_length(), Some(3));
    }

    #[tokio::test]
    async fn incoming_debug_format() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"dbg").await;
        let body = builder.incoming(incoming);
        let debug = format!("{body:?}");
        assert!(debug.contains("Incoming"));
    }

    #[tokio::test]
    async fn incoming_buffered_then_clone() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"clone me").await;
        let body = builder.incoming(incoming);

        let buffered = body.into_buffered().await.unwrap();
        let cloned = buffered.try_clone().unwrap();

        assert_eq!(buffered.into_text().await.unwrap(), "clone me");
        assert_eq!(cloned.into_text().await.unwrap(), "clone me");
    }

    #[tokio::test]
    async fn incoming_with_buffer_limit_exceeded() {
        let builder = HttpBodyBuilder::new_fake().with_response_buffer_limit(Some(5));
        let incoming = crate::testing::create_incoming(b"this exceeds the limit").await;
        let body = builder.incoming(incoming);

        let err = body.into_buffered().await.unwrap_err();
        assert!(err.to_string().contains("body size exceeds the limit"));
    }

    #[tokio::test]
    async fn incoming_with_buffer_limit_ok() {
        let builder = HttpBodyBuilder::new_fake().with_response_buffer_limit(Some(1024));
        let incoming = crate::testing::create_incoming(b"fits").await;
        let body = builder.incoming(incoming);

        let text = body.into_text().await.unwrap();
        assert_eq!(text, "fits");
    }

    #[tokio::test]
    async fn incoming_is_end_stream_before_consume() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"data").await;
        let body = builder.incoming(incoming);
        // A fresh incoming body with content is not at end-of-stream.
        assert!(!body.is_end_stream());
    }

    #[tokio::test]
    async fn incoming_empty_is_end_stream() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"").await;
        let body = builder.incoming(incoming);
        // An empty incoming body should signal end-of-stream.
        assert!(body.is_end_stream());
    }

    #[tokio::test]
    async fn incoming_poll_frame_yields_correct_data() {
        let builder = HttpBodyBuilder::new_fake();
        let incoming = crate::testing::create_incoming(b"exact").await;
        let mut body = pin!(builder.incoming(incoming));
        let mut cx = Context::from_waker(Waker::noop());

        if let Poll::Ready(Some(Ok(frame))) = body.as_mut().poll_frame(&mut cx) {
            let data = frame.into_data().unwrap();
            assert_eq!(data, b"exact");
        } else {
            panic!("expected a data frame");
        }
    }

    #[test]
    fn is_empty_for_empty_body() {
        let builder = HttpBodyBuilder::new_fake();
        assert!(builder.empty().is_empty());
    }

    #[test]
    fn is_empty_for_text_body() {
        let builder = HttpBodyBuilder::new_fake();
        assert!(!builder.text("hello").is_empty());
    }

    #[test]
    fn is_empty_for_zero_length_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        assert!(builder.bytes(BytesView::new()).is_empty());
    }

    #[test]
    fn stream_body_creation() {
        let builder = HttpBodyBuilder::new_fake();
        let chunks = vec![
            Ok(BytesView::copied_from_slice(b"hello ", &builder)),
            Ok(BytesView::copied_from_slice(b"world", &builder)),
        ];
        let body = builder.stream(futures::stream::iter(chunks));

        // Streams don't have a known content length
        assert_eq!(body.content_length(), None);

        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn stream_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.stream(futures::stream::iter(Vec::<Result<BytesView>>::new()));

        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn into_stream_produces_chunks() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("stream test");

        let chunks: Vec<_> = block_on(body.into_stream().try_collect()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], b"stream test");
    }

    #[test]
    fn has_memory_returns_usable_provider() {
        let builder = HttpBodyBuilder::new_fake();

        let memory = builder.memory();
        let buf = memory.reserve(64);

        assert!(buf.capacity() >= 64);
    }
}
