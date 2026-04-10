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

use bytesbuf::BytesView;
use futures::{Stream, TryStreamExt};
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use pin_project::pin_project;
use thread_aware::ThreadAware;

use crate::constants::DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES;
use crate::{HttpError, Result};

mod builder;
pub(crate) mod options;
pub use builder::HttpBodyBuilder;
pub use options::BodyOptions;

pub(crate) mod timeout_body;

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
/// # let builder = HttpBodyBuilder::new_fake();
/// // Create different body types
/// let text_body = builder.text("Hello world");
/// let binary_body = builder.slice(&[1, 2, 3, 4]);
/// let empty_body = builder.empty();
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
/// You can customize this limit via [`BodyOptions::buffer_limit`].
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
/// use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
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
/// # #[tokio::main]
/// # async fn main() {
/// #     let path = std::env::temp_dir().join("http_extensions_doctest");
/// #     let mut file = std::fs::File::create(&path).unwrap();
/// #     download_to_file(HttpBodyBuilder::new_fake().text("test"), &mut file).await.unwrap();
/// #     std::fs::remove_file(path).ok();
/// # }
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
    /// # use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     let body_bytes = body.into_bytes().await?;
    ///     println!("Received {} bytes", body_bytes.len());
    ///     Ok(())
    /// }
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     example(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
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
    /// # use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     let text = body.into_text().await?;
    ///     println!("Received: {}", text);
    ///     Ok(())
    /// }
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     example(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
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
    /// To change the memory limit, use [`BodyOptions::buffer_limit`] when constructing the builder.
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
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     example(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
    /// ```
    pub async fn into_buffered(self) -> Result<Self> {
        let builder = self.builder;

        match self.kind {
            Kind::Bytes(Some(data)) => Ok(builder.bytes(data)),
            Kind::Bytes(None) => Err(HttpError::validation("body cannot be buffered because it is already consumed")),
            Kind::Empty => Ok(builder.empty()),
            Kind::Body(b, options) => {
                let limit = options.buffer_limit;
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
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     let body = HttpBodyBuilder::new_fake()
    /// #         .text(r#"{"id": 1, "name": "Alice", "is_active": true}"#);
    /// #     example(body).await.unwrap();
    /// # }
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
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     let body = HttpBodyBuilder::new_fake()
    /// #         .text(r#"{"id": 1, "name": "Alice", "email": "alice@example.com", "is_active": true}"#);
    /// #     example(body).await.unwrap();
    /// # }
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let text_body = builder.text("Hello, world!");
    /// assert_eq!(text_body.content_length(), Some(13));
    ///
    /// let empty_body = builder.empty();
    /// assert_eq!(empty_body.content_length(), Some(0));
    /// ```
    #[must_use]
    pub fn content_length(&self) -> Option<u64> {
        match &self.kind {
            Kind::Bytes(Some(bytes)) => Some(bytes.len() as u64),
            Kind::Bytes(None) | Kind::Empty => Some(0),
            Kind::Body(b, _) => b.size_hint().exact(),
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
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let empty_body = builder.empty();
    /// assert!(empty_body.is_empty());
    ///
    /// let text_body = builder.text("Hello");
    /// assert!(!text_body.is_empty());
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
            Kind::Body(..) | Kind::Bytes(None) => None,
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
    /// use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
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
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     process_body(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
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
            BodyInnerProj::Bytes(bytes) => bytes
                .take()
                .map_or_else(|| Ready(None), |bytes| Ready((!bytes.is_empty()).then(|| Ok(Frame::data(bytes))))),
            BodyInnerProj::Empty => Ready(None),
            BodyInnerProj::Body(body, _) => body.as_mut().poll_frame(cx),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            Kind::Bytes(Some(bytes)) => SizeHint::with_exact(bytes.len() as u64),
            Kind::Bytes(None) | Kind::Empty => SizeHint::with_exact(0),
            Kind::Body(b, _) => b.size_hint(),
        }
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            Kind::Bytes(Some(x)) => x.is_empty(),
            Kind::Bytes(None) | Kind::Empty => true,
            Kind::Body(b, _) => b.is_end_stream(),
        }
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "BytesView is intentionally large, though future optimizations may decrease size"
)]
#[pin_project(project = BodyInnerProj)]
enum Kind {
    Bytes(Option<BytesView>),
    Empty,
    Body(Pin<Box<dyn Body<Data = BytesView, Error = HttpError> + Send>>, BodyOptions),
}

impl Debug for Kind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(_) => f.debug_struct("Bytes").finish(),
            Self::Empty => f.debug_struct("Empty").finish(),
            Self::Body(_, _) => f.debug_struct("Body").finish(),
        }
    }
}

async fn collect_with_limit(mut data: impl Stream<Item = Result<BytesView>> + Send + Unpin, limit: Option<usize>) -> Result<BytesView> {
    let mut total_size = 0_usize;
    let mut fragments = Vec::new();
    let limit = limit.unwrap_or(DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES);

    while let Some(bytes) = data.try_next().await? {
        total_size = check_size_limit(total_size, bytes.len(), limit)?;
        fragments.push(bytes);
    }

    Ok(BytesView::from_views(fragments))
}

fn check_size_limit(current_size: usize, additional: usize, limit: usize) -> Result<usize> {
    let total = current_size
        .checked_add(additional)
        .ok_or_else(|| HttpError::validation(format!("body size exceeds the limit of {limit} bytes")))?;

    if total > limit {
        return Err(HttpError::validation(format!("body size exceeds the limit of {limit} bytes")));
    }

    Ok(total)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::task::Waker;

    use bytes::Bytes;
    use bytesbuf::mem::GlobalPool;
    use futures::executor::block_on;
    use http_body_util::StreamBody;
    use ohno::ErrorExt;
    use serde::{Deserialize, Serialize};
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::testing::create_stream_body;

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
        let body = HttpBodyBuilder::new_fake().body(http_body_util::Empty::new(), &BodyOptions::default());

        assert!(body.try_clone().is_none());

        let body = block_on(body.into_buffered()).unwrap();
        assert_eq!(Some(0), body.content_length());
    }

    #[test]
    fn into_buffered_already_consumed_body_returns_error() {
        let mut body = HttpBodyBuilder::new_fake().text("hello");

        // Consume the body bytes via poll_frame, which sets Kind::Bytes to None.
        let _frame = block_on(body.frame());

        // Now the body is consumed; into_buffered should fail.
        let err = block_on(body.into_buffered()).unwrap_err();
        assert!(
            err.to_string().contains("body cannot be buffered because it is already consumed"),
            "expected consumed body error, got: {err}"
        );
    }

    #[test]
    fn body_error_propagation() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.body(
            StreamBody::new(futures::stream::once(async { Err(HttpError::validation("test error")) })),
            &BodyOptions::default(),
        );

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
        let custom_body = builder.body(http_body_util::Empty::new(), &BodyOptions::default());

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
    fn into_bytes_custom_body_fails() {
        let builder = HttpBodyBuilder::new_fake();
        let custom_body = builder.body(http_body_util::Empty::new(), &BodyOptions::default());

        BytesView::try_from(custom_body).unwrap_err();
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
    fn check_size_limit_overflow_returns_error() {
        let result = check_size_limit(usize::MAX, 1, usize::MAX);

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("body size exceeds the limit"),
            "expected body size error, got: {err}"
        );
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
        let body = builder.stream(stream, &BodyOptions::default());
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
        let mut body = pin!(HttpBodyBuilder::new_fake().empty());
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
        let body = builder.body(custom, &BodyOptions::default());

        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_custom_body_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let data = Bytes::from_static(b"test data");
        let custom = http_body_util::Full::new(data.into());
        let body = builder.body(custom, &BodyOptions::default());

        assert!(!body.is_end_stream());
    }

    // ── Incoming (real hyper body) tests ─────────────────────────────────

    #[test]
    fn external_body_into_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"raw bytes", &BodyOptions::default());
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"raw bytes");
    }

    #[test]
    fn external_body_empty_into_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"", &BodyOptions::default());
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn external_body_into_json_owned() {
        let builder = HttpBodyBuilder::new_fake();
        let json_bytes = br#"{"id":42,"name":"alice"}"#;
        let body = create_stream_body(&builder, json_bytes, &BodyOptions::default());
        let model: Model = block_on(body.into_json_owned()).unwrap();
        assert_eq!(
            model,
            Model {
                id: 42,
                name: "alice".to_string()
            }
        );
    }

    #[test]
    fn external_body_try_clone_returns_none() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"no clone", &BodyOptions::default());
        assert!(body.try_clone().is_none());
    }

    #[test]
    fn external_body_into_bytes_view_fails() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"not buffered", &BodyOptions::default());
        BytesView::try_from(body).unwrap_err();
    }

    #[test]
    fn external_body_into_bytes_no_buffering_returns_none() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"data", &BodyOptions::default());
        assert!(body.into_bytes_no_buffering().is_none());
    }

    #[test]
    fn external_body_buffered_then_clone() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"clone me", &BodyOptions::default());

        let buffered = block_on(body.into_buffered()).unwrap();
        let cloned = buffered.try_clone().unwrap();

        assert_eq!(block_on(buffered.into_text()).unwrap(), "clone me");
        assert_eq!(block_on(cloned.into_text()).unwrap(), "clone me");
    }

    #[test]
    fn external_body_with_buffer_limit_exceeded() {
        let builder = HttpBodyBuilder::new_fake().with_options(BodyOptions::default().buffer_limit(5));
        let body = create_stream_body(&builder, b"this exceeds the limit", &BodyOptions::default());

        let err = block_on(body.into_buffered()).unwrap_err();
        assert!(err.to_string().contains("body size exceeds the limit"));
    }

    #[test]
    fn external_body_with_buffer_limit_ok() {
        let builder = HttpBodyBuilder::new_fake().with_options(BodyOptions::default().buffer_limit(1024));
        let body = create_stream_body(&builder, b"fits", &BodyOptions::default());

        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "fits");
    }

    #[test]
    fn external_body_is_end_stream_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"data", &BodyOptions::default());
        // A stream body with content is not at end-of-stream.
        assert!(!body.is_end_stream());
    }

    #[test]
    fn external_body_poll_frame_yields_correct_data() {
        let builder = HttpBodyBuilder::new_fake();
        let mut body = pin!(create_stream_body(&builder, b"exact", &BodyOptions::default()));
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
    fn into_stream_produces_chunks() {
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("stream test");

        let chunks: Vec<_> = block_on(body.into_stream().try_collect()).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], b"stream test");
    }
}
