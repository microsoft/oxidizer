// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP body types for requests and responses.
//!
//! [`HttpBody`] supports:
//!
//! - Text and string data.
//! - Binary data.
//! - JSON data with the `json` feature.
//! - Streaming content.
//! - Empty bodies.
//! - Custom body implementations.
//!
//! Create bodies with [`HttpBodyBuilder`].
//! All bodies implement the standard `http_body::Body` trait.

use std::fmt::{Debug, Formatter};
use std::future::ready;
use std::io::Read;
use std::pin::Pin;
use std::task::Poll::Ready;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use futures::future::Either;
use futures::{FutureExt, Stream, TryFutureExt, TryStreamExt};
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use pin_project::pin_project;
use thread_aware::ThreadAware;

use crate::constants::DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES;
use crate::error_labels::{LABEL_BODY_CONSUMED, LABEL_BODY_NOT_BUFFERED, LABEL_BODY_SIZE_LIMIT, LABEL_BODY_UTF8_INVALID};
use crate::{HttpError, Result};

mod builder;
pub(crate) mod options;
pub use builder::HttpBodyBuilder;
pub use options::HttpBodyOptions;

pub(crate) mod timeout_body;

/// An HTTP body that can contain several data types.
///
/// `HttpBody` supports text, binary data, JSON, and streaming content.
/// JSON requires the `json` feature.
/// Use [`HttpBodyBuilder`] to create bodies.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "test-util")] {
/// # use http_extensions::HttpBodyBuilder;
/// # let builder = HttpBodyBuilder::new_fake();
/// // Create different body types.
/// let text_body = builder.text("Hello world");
/// let binary_body = builder.slice(&[1, 2, 3, 4]);
/// let empty_body = builder.empty();
/// # }
/// # }
/// ```
///
/// # How does `HttpBody` work?
///
/// [`HttpBody`] implements the [`http_body::Body`] trait.
/// The trait processes HTTP data as it arrives:
///
/// 1. Data arrives in individual [`http_body::Frame`] values.
///    You do not have to wait for the complete response.
/// 2. Each chunk is a [`bytesbuf::BytesView`].
///    The memory pool reuses its memory after the chunk is consumed.
/// 3. [`bytesbuf::BytesView`] avoids data copies when possible.
///
/// [`HttpBody::poll_frame`] returns a [`Poll`] value when data is available.
/// After you consume data from a [`BytesView`], the memory returns to the pool.
///
/// # Buffering
///
/// [`HttpBody::into_buffered`] loads the complete body into memory.
/// It also releases the underlying network connection.
/// Use buffering to:
///
/// - Process the same data multiple times
/// - Clone a streaming body before it is consumed
/// - Keep data after the network connection closes
///
/// Buffering uses a memory limit to prevent an out-of-memory failure.
/// The default limit is 2 GB.
/// If a response exceeds the limit, the operation returns an error.
/// Set a different limit with [`HttpBodyOptions::buffer_limit`].
///
/// # Streaming
///
/// A stream processes each data chunk when it arrives.
/// Use a stream for large response bodies, such as file downloads.
/// A stream has no buffering limit because it does not store the complete body.
///
/// The following example writes a downloaded file to disk:
///
/// ```
/// use futures::TryStreamExt; // Provides stream operations.
/// use http_body_util::BodyExt; // Provides into_data_stream().
/// use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
///
/// async fn download_to_file(
///     body: HttpBody,
///     output_file: &mut std::fs::File,
/// ) -> Result<(), HttpError> {
///     // Convert the response into a stream.
///     let mut stream = body.into_data_stream();
///
///     // Process each chunk when it arrives.
///     while let Some(mut data) = stream.try_next().await? {
///         // Write the chunk to the output file. BytesView implements Read.
///         std::io::copy(&mut data, output_file)?;
///     }
///
///     Ok(())
/// }
/// # #[cfg(not(feature = "test-util"))] fn main() {}
/// # #[cfg(feature = "test-util")]
/// # #[tokio::main]
/// # async fn main() {
/// #     let path = std::env::temp_dir().join("http_extensions_doctest");
/// #     let mut file = std::fs::File::create(&path).unwrap();
/// #     download_to_file(HttpBodyBuilder::new_fake().text("test"), &mut file).await.unwrap();
/// #     std::fs::remove_file(path).ok();
/// # }
/// ```
///
/// Call [`into_data_stream`][http_body_util::BodyExt::into_data_stream] to convert
/// `HttpBody` into a [`Stream`][futures::Stream].
/// You can then use the extension methods for [`Stream`][futures::Stream].
///
/// > [!NOTE]
/// > The example uses [`futures`] for stream processing.
/// > It uses [`http_body_util`] to convert the body into a data stream.
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
}

// HttpBody implementations.

impl HttpBody {
    /// Creates an empty body (zero bytes).
    pub(crate) const fn empty() -> Self {
        Self { kind: Kind::Empty }
    }

    /// Creates a body backed by an in-memory byte sequence.
    pub(crate) fn from_bytes(bytes: BytesView) -> Self {
        Self {
            kind: Kind::Bytes(Some(bytes)),
        }
    }

    /// The policies this body was created with.
    ///
    /// A buffered body has none of its own: it has nothing left to time out and
    /// nothing left to buffer.
    pub(crate) const fn options(&self) -> HttpBodyOptions {
        match &self.kind {
            Kind::Body(_, options) => *options,
            Kind::Bytes(_) | Kind::Empty => HttpBodyOptions::new(),
        }
    }

    /// Creates a streaming body backed by a boxed [`Body`] implementation.
    pub(crate) fn from_streaming(body: Pin<Box<dyn Body<Data = BytesView, Error = HttpError> + Send>>, options: HttpBodyOptions) -> Self {
        Self {
            kind: Kind::Body(body, options),
        }
    }

    /// Converts the body into a memory-efficient view over a byte sequence.
    ///
    /// Useful when you need direct access to the raw bytes without extra conversions.
    ///
    /// # Errors
    ///
    /// Returns an error if the body cannot be collected or was already consumed.
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
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     example(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
    /// ```
    pub fn into_bytes(self) -> impl Future<Output = Result<BytesView>> + Send {
        self.into_buffered().map_ok(|body| {
            body.into_bytes_no_buffering()
                .unwrap_or_else(|| unreachable!("once body is buffered, it must be a view over a byte sequence"))
        })
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
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     example(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
    /// ```
    #[expect(clippy::cast_possible_truncation, reason = "size_hint is used for capacity, not exact size")]
    pub fn into_text(self) -> impl Future<Output = Result<String>> + Send {
        let capacity = self.size_hint().lower() as usize;

        self.into_bytes().map(move |result| {
            let mut text = String::with_capacity(capacity);
            result?
                .read_to_string(&mut text)
                .map_err(|e| HttpError::validation_with_label(format!("body contains invalid UTF-8: {e}"), LABEL_BODY_UTF8_INVALID))?;

            Ok(text)
        })
    }

    /// Loads the entire body into memory for easier handling.
    ///
    /// Use this method to:
    ///
    /// - Read a streaming body multiple times
    /// - Clone a body that was not cloneable before buffering
    /// - Preload data to avoid connection issues later
    ///
    /// Set the memory limit with [`HttpBodyOptions::buffer_limit`] on the builder.
    /// The default limit is 2 GB.
    ///
    /// # Caveats
    ///
    /// Do not use this method for an unbounded body.
    /// It loads the complete body into memory.
    ///
    /// # Errors
    ///
    /// - A network problem stops data collection.
    /// - Another operation already consumed the body.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_extensions::{HttpBody, HttpBodyBuilder, HttpError};
    ///
    /// async fn example(body: HttpBody) -> Result<(), HttpError> {
    ///     // Load the complete body into memory.
    ///     let buffered = body.into_buffered().await?;
    ///
    ///     // The buffered body can be used more than once.
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     example(HttpBodyBuilder::new_fake().text("test")).await.unwrap();
    /// # }
    /// ```
    pub fn into_buffered(self) -> impl Future<Output = Result<Self>> + Send {
        match self.kind {
            Kind::Bytes(Some(data)) => Either::Left(ready(Ok(Self::from_bytes(data)))),
            Kind::Bytes(None) => Either::Left(ready(Err(HttpError::validation_with_label(
                "body cannot be buffered because it is already consumed",
                LABEL_BODY_CONSUMED,
            )))),
            Kind::Empty => Either::Left(ready(Ok(Self::empty()))),
            Kind::Body(b, options) => {
                let limit = options.buffer_limit;
                Either::Right(collect_with_limit(b.into_data_stream(), limit).map_ok(Self::from_bytes))
            }
        }
    }

    /// Consumes the body and converts it to JSON.
    ///
    /// Converts the body directly to your desired type. Available with the `json` feature.
    ///
    /// # Errors
    ///
    /// Returns an error if the body cannot be collected or JSON parsing fails.
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
    ///     let user: User = body.into_json().await?;
    ///
    ///     println!("Received user: {} (ID: {})", user.name, user.id);
    ///
    ///     // Process the user data...
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     let body = HttpBodyBuilder::new_fake()
    /// #         .text(r#"{"id": 1, "name": "Alice", "is_active": true}"#);
    /// #     example(body).await.unwrap();
    /// # }
    /// ```
    #[cfg(any(feature = "json", test))]
    pub fn into_json<T: serde_core::de::DeserializeOwned>(self) -> impl Future<Output = Result<T>> + Send {
        self.into_json_ref::<T>().map(|json| Ok(json?.read_owned()?))
    }

    /// Consumes the body and creates a JSON parser that can borrow data.
    ///
    /// The parser reads directly from the body buffer.
    /// Unlike [`into_json`][HttpBody::into_json], it supports borrowed data
    /// and types that use `Cow<str>`.
    ///
    /// The returned [`Json<T>`][crate::Json] can deserialize types that borrow
    /// from the original data.
    ///
    /// # Errors
    ///
    /// Returns an error if the body cannot be collected into a sequence.
    /// JSON parsing errors occur when you call methods on the returned [`Json<T>`][crate::Json].
    ///
    /// # Examples
    ///
    /// The following example borrows strings:
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
    ///     // Parse JSON and permit borrowed strings.
    ///     let mut json = body.into_json_ref::<User>().await?;
    ///     let user = json.read()?;
    ///
    ///     println!("User: {} <{}> (ID: {})", user.name, user.email, user.id);
    ///
    ///     // The name and email fields can borrow from the original JSON buffer.
    ///     // This avoids string allocations.
    ///     Ok(())
    /// }
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
    /// # #[tokio::main]
    /// # async fn main() {
    /// #     let body = HttpBodyBuilder::new_fake()
    /// #         .text(r#"{"id": 1, "name": "Alice", "email": "alice@example.com", "is_active": true}"#);
    /// #     example(body).await.unwrap();
    /// # }
    /// ```
    ///
    /// For types that do not borrow data, use [`into_json`][HttpBody::into_json].
    #[cfg(any(feature = "json", test))]
    pub fn into_json_ref<'a, T: serde_core::de::Deserialize<'a>>(self) -> impl Future<Output = Result<crate::json::Json<T>>> + Send {
        self.into_bytes().map_ok(crate::json::Json::<T>::new)
    }

    /// Gets the body's content length in bytes, if known.
    ///
    /// Returns the size without consuming the body.
    ///
    /// Use the result to set `Content-Length` headers or allocate buffers.
    ///
    /// Returns `Some(size)` for known-length bodies, `None` for streaming bodies with unknown length.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::{HttpBodyBuilder, HttpBody};
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let text_body = builder.text("Hello, world!");
    /// assert_eq!(text_body.content_length(), Some(13));
    ///
    /// let empty_body = builder.empty();
    /// assert_eq!(empty_body.content_length(), Some(0));
    /// # }
    /// # }
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
    /// This method returns `true` when the content length is exactly zero.
    /// For streaming bodies with unknown length, this returns `false`.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use http_extensions::HttpBodyBuilder;
    /// # let builder = HttpBodyBuilder::new_fake();
    /// let empty_body = builder.empty();
    /// assert!(empty_body.is_empty());
    ///
    /// let text_body = builder.text("Hello");
    /// assert!(!text_body.is_empty());
    /// # }
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
    /// to load the complete content into memory first.
    /// Large bodies can then use a large amount of memory.
    #[must_use]
    pub fn try_clone(&self) -> Option<Self> {
        match &self.kind {
            Kind::Bytes(Some(bytes)) => Some(Self::from_bytes(bytes.clone())),
            Kind::Empty => Some(Self::empty()),
            Kind::Body(..) | Kind::Bytes(None) => None,
        }
    }

    /// Converts this body into a stream of byte chunks.
    ///
    /// This method wraps
    /// [`BodyExt::into_data_stream()`][http_body_util::BodyExt::into_data_stream].
    /// Callers do not need to import `http_body_util::BodyExt`.
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
    /// # #[cfg(not(feature = "test-util"))] fn main() {}
    /// # #[cfg(feature = "test-util")]
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
            _ => Err(HttpError::validation_with_label(
                "body cannot be converted to byte sequence because it is not buffered",
                LABEL_BODY_NOT_BUFFERED,
            )),
        }
    }
}

/// Implements the standard `http_body::Body` interface for [`HttpBody`].
///
/// This interface lets HTTP clients and servers use [`HttpBody`].
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
    reason = "BytesView is intentionally large, but a future optimization can reduce its size"
)]
#[pin_project(project = BodyInnerProj)]
enum Kind {
    Bytes(Option<BytesView>),
    Empty,
    Body(Pin<Box<dyn Body<Data = BytesView, Error = HttpError> + Send>>, HttpBodyOptions),
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

fn collect_with_limit(
    data: impl Stream<Item = Result<BytesView>> + Send + Unpin,
    limit: Option<usize>,
) -> impl Future<Output = Result<BytesView>> + Send {
    let limit = limit.unwrap_or(DEFAULT_RESPONSE_BUFFER_LIMIT_BYTES);

    data.try_fold((0_usize, Vec::new()), move |(total_size, mut fragments), bytes| {
        let result = check_size_limit(total_size, bytes.len(), limit).map(|total_size| {
            fragments.push(bytes);
            (total_size, fragments)
        });
        ready(result)
    })
    .map_ok(|(_, fragments)| BytesView::from_views(fragments))
}

fn check_size_limit(current_size: usize, additional: usize, limit: usize) -> Result<usize> {
    let total = current_size
        .checked_add(additional)
        .ok_or_else(|| HttpError::validation_with_label(format!("body size exceeds the limit of {limit} bytes"), LABEL_BODY_SIZE_LIMIT))?;

    if total > limit {
        return Err(HttpError::validation_with_label(
            format!("body size exceeds the limit of {limit} bytes"),
            LABEL_BODY_SIZE_LIMIT,
        ));
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
    use ohno::{ErrorExt, Labeled};
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

        let result: Model = block_on(body.into_json()).unwrap();

        assert_eq!(data, result);
    }

    #[test]
    fn json_deserialization_error() {
        // Create a body with invalid JSON content
        let builder = HttpBodyBuilder::new_fake();
        let body = builder.text("{invalid json}");

        // Deserialization into this model is expected to fail.
        let result: Result<Model> = block_on(body.into_json());
        result.unwrap_err();
    }

    #[test]
    fn into_json_ref_with_cow_strings() {
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

        let mut json_result = block_on(body.into_json_ref::<User>()).unwrap();
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
        let body = HttpBodyBuilder::new_fake().body(http_body_util::Empty::new(), &HttpBodyOptions::default());

        assert!(body.try_clone().is_none());

        let body = block_on(body.into_buffered()).unwrap();
        assert_eq!(Some(0), body.content_length());
    }

    #[test]
    fn into_buffered_already_consumed_body_returns_error() {
        let mut body = HttpBodyBuilder::new_fake().text("hello");

        // Consume the body bytes via poll_frame, which sets Kind::Bytes to None.
        let _frame = block_on(body.frame());

        // The body is now consumed. into_buffered is expected to fail.
        let err = block_on(body.into_buffered()).unwrap_err();
        assert_eq!(err.label(), "body_consumed");
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
            &HttpBodyOptions::default(),
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
        assert_eq!(err.label(), "body_size_limit");
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
        assert_eq!(error.label(), "body_utf8_invalid");
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
        let custom_body = builder.body(http_body_util::Empty::new(), &HttpBodyOptions::default());

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
        let custom_body = builder.body(http_body_util::Empty::new(), &HttpBodyOptions::default());

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
        assert_eq!(err.label(), "body_size_limit");
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
        let body = builder.stream(stream, &HttpBodyOptions::default());
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
        let body = builder.body(custom, &HttpBodyOptions::default());

        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_custom_body_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let data = Bytes::from_static(b"test data");
        let custom = http_body_util::Full::new(data.into());
        let body = builder.body(custom, &HttpBodyOptions::default());

        assert!(!body.is_end_stream());
    }

    // ── Incoming (real hyper body) tests ─────────────────────────────────

    #[test]
    fn external_body_into_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"raw bytes", &HttpBodyOptions::default());
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"raw bytes");
    }

    #[test]
    fn external_body_empty_into_bytes() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"", &HttpBodyOptions::default());
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn external_body_into_json() {
        let builder = HttpBodyBuilder::new_fake();
        let json_bytes = br#"{"id":42,"name":"alice"}"#;
        let body = create_stream_body(&builder, json_bytes, &HttpBodyOptions::default());
        let model: Model = block_on(body.into_json()).unwrap();
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
        let body = create_stream_body(&builder, b"no clone", &HttpBodyOptions::default());
        assert!(body.try_clone().is_none());
    }

    #[test]
    fn external_body_into_bytes_view_fails() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"not buffered", &HttpBodyOptions::default());
        let err = BytesView::try_from(body).unwrap_err();
        assert_eq!(err.label(), "body_not_buffered");
    }

    #[test]
    fn external_body_into_bytes_no_buffering_returns_none() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"data", &HttpBodyOptions::default());
        assert!(body.into_bytes_no_buffering().is_none());
    }

    #[test]
    fn external_body_buffered_then_clone() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"clone me", &HttpBodyOptions::default());

        let buffered = block_on(body.into_buffered()).unwrap();
        let cloned = buffered.try_clone().unwrap();

        assert_eq!(block_on(buffered.into_text()).unwrap(), "clone me");
        assert_eq!(block_on(cloned.into_text()).unwrap(), "clone me");
    }

    #[test]
    fn external_body_with_buffer_limit_exceeded() {
        let builder = HttpBodyBuilder::new_fake().with_options(HttpBodyOptions::default().buffer_limit(5));
        let body = create_stream_body(&builder, b"this exceeds the limit", &HttpBodyOptions::default());

        let err = block_on(body.into_buffered()).unwrap_err();
        assert_eq!(err.label(), "body_size_limit");
        assert!(err.to_string().contains("body size exceeds the limit"));
    }

    #[test]
    fn external_body_with_buffer_limit_ok() {
        let builder = HttpBodyBuilder::new_fake().with_options(HttpBodyOptions::default().buffer_limit(1024));
        let body = create_stream_body(&builder, b"fits", &HttpBodyOptions::default());

        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "fits");
    }

    #[test]
    fn external_body_is_end_stream_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"data", &HttpBodyOptions::default());
        // A stream body with content is not at end-of-stream.
        assert!(!body.is_end_stream());
    }

    #[test]
    fn external_body_poll_frame_yields_correct_data() {
        let builder = HttpBodyBuilder::new_fake();
        let mut body = pin!(create_stream_body(&builder, b"exact", &HttpBodyOptions::default()));
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
