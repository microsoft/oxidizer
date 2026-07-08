// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Response value types produced by the transcode path.
//!
//! The generated transcode (`try_transcode` / `transcode`, emitted by
//! `rest_over_grpc::build`) yields a [`TranscodeResponse`]: a unary RPC is a
//! buffered [`HttpResponse`], while a server-streaming RPC becomes a
//! [`StreamingResponse`] whose encoded frames reach the wire incrementally rather
//! than being buffered first.
//!
//! These are the framework-neutral value types; a serving adapter
//! ([`serve_http`](crate::serving::serve_http) /
//! [`RestService`](crate::serving::RestService)) turns a
//! [`TranscodeResponse`] into an [`http::Response`] with a
//! [`http_body::Body`] — buffered for a unary reply, streaming for a
//! server-streaming one.

use core::fmt;
use core::pin::Pin;

use serde::Serialize;

use crate::handling::Status;
use crate::stream::{Stream, StreamEncoding, encode_frames};
use crate::transcoding::HttpResponse;

/// A boxed stream of already-encoded response body frames.
///
/// Each item is one encoding frame (a JSON-array element, an NDJSON line, or an
/// SSE `data:` envelope); an [`Err`] terminates the stream with a [`Status`].
pub type FrameStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, Status>> + Send>>;

/// A boxed, `'static` stream of a server-streaming RPC's response messages.
///
/// This is the return type a server-streaming handler yields once initiation
/// succeeds: `async fn(&self, request, cx) -> Result<ResponseStream<T>, Status>`.
/// Boxing keeps the stream `'static` (independent of the `&self` / `Context`
/// borrows), so it can be forwarded to the wire by a streaming adapter after the
/// transcode call returns. Build one with [`Box::pin`].
///
/// # Examples
///
/// ```
/// # fn main() {
/// # {
/// use futures_util::stream;
/// use rest_over_grpc::handling::{ResponseStream, Status};
///
/// fn ticks() -> ResponseStream<u32> {
///     Box::pin(stream::iter(vec![Ok::<_, Status>(1), Ok(2)]))
/// }
/// # let _ = ticks();
/// # }
/// # }
/// ```
pub type ResponseStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// A server-streaming response whose body frames are produced incrementally.
///
/// Unlike the buffered [`HttpResponse`], a `StreamingResponse` holds a live
/// stream of encoded frames, so a streaming adapter can forward each frame to
/// the client as it is produced. The HTTP status is always `200 OK` — the
/// response line and headers are sent before any handler item is observed, so a
/// mid-stream failure can only truncate the body (surfaced as a transport error
/// by the adapter), not change the status. A failure that happens *before*
/// streaming (for example, decoding the request) is reported on the unary path
/// as an [`HttpResponse`] instead.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # {
/// use futures_util::stream;
/// use rest_over_grpc::codegen_helpers::StreamEncoding;
/// use rest_over_grpc::handling::Status;
/// use rest_over_grpc::transcoding::StreamingResponse;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Message {
///     n: u32,
/// }
///
/// let items = stream::iter(vec![Ok::<_, Status>(Message { n: 1 })]);
/// let response = StreamingResponse::encode(items, StreamEncoding::NdJson);
/// assert_eq!(response.content_type(), "application/x-ndjson");
/// # }
/// # }
/// ```
pub struct StreamingResponse {
    content_type: &'static str,
    headers: http::HeaderMap,
    frames: FrameStream,
}

impl StreamingResponse {
    /// Builds a streaming response from an already-encoded stream of body
    /// `frames`, served with `content_type`.
    ///
    /// Most callers want [`StreamingResponse::encode`], which frames a stream of
    /// handler messages in a negotiated [`StreamEncoding`]. This lower-level
    /// constructor is for adapters that produce their own framing.
    #[must_use]
    pub fn new<S>(content_type: &'static str, frames: S) -> Self
    where
        S: Stream<Item = Result<Vec<u8>, Status>> + Send + 'static,
    {
        Self {
            content_type,
            headers: http::HeaderMap::new(),
            frames: Box::pin(frames),
        }
    }

    /// Builds a streaming response by framing a stream of handler messages in
    /// the negotiated `encoding`.
    ///
    /// Keeps the sequence lazy so each frame can be flushed as it is produced,
    /// rather than buffering the whole response into an [`HttpResponse`] first.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # {
    /// use futures_util::stream;
    /// use rest_over_grpc::codegen_helpers::StreamEncoding;
    /// use rest_over_grpc::handling::Status;
    /// use rest_over_grpc::transcoding::StreamingResponse;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Message {
    ///     n: u32,
    /// }
    ///
    /// let items = stream::iter(vec![Ok::<_, Status>(Message { n: 7 })]);
    /// let response = StreamingResponse::encode(items, StreamEncoding::Sse);
    /// assert_eq!(response.content_type(), "text/event-stream");
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn encode<S, T>(items: S, encoding: StreamEncoding) -> Self
    where
        S: Stream<Item = Result<T, Status>> + Send + 'static,
        T: Serialize + 'static,
    {
        Self::new(encoding.content_type(), encode_frames(items, encoding))
    }

    /// The `Content-Type` header value for this response's encoding.
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        self.content_type
    }

    /// The custom response headers to send with this streaming response
    /// (excluding the authoritative `Content-Type`).
    #[must_use]
    pub const fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// Merges `headers` into this response's custom headers, preserving repeated
    /// values.
    ///
    /// Used by the generated streaming transcoder to apply the response headers a
    /// handler set on its [`Context`](crate::handling::Context). The headers are sent
    /// before the first body frame, so — unlike the buffered path — they must be
    /// set by the time the handler returns its stream.
    pub fn merge_headers(&mut self, headers: http::HeaderMap) {
        crate::context::append_headers(&mut self.headers, headers);
    }

    /// Consumes the response, returning its stream of encoded body frames.
    #[must_use]
    pub fn into_frames(self) -> FrameStream {
        self.frames
    }

    /// Consumes the response, returning its content type, custom headers, and
    /// stream of encoded body frames.
    #[must_use]
    pub fn into_parts(self) -> (&'static str, http::HeaderMap, FrameStream) {
        (self.content_type, self.headers, self.frames)
    }
}

impl fmt::Debug for StreamingResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingResponse")
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

/// The result of the transcode path: a unary RPC's buffered
/// [`HttpResponse`] or a server-streaming RPC's [`StreamingResponse`].
///
/// The `rest_over_grpc::build`-generated `try_transcode` / `transcode` return
/// this so a single transcode call serves both RPC shapes, and a serving adapter
/// can render each variant with the appropriate [`http_body::Body`].
///
/// # Examples
///
/// ```
/// # fn main() {
/// # {
/// use rest_over_grpc::transcoding::{HttpResponse, TranscodeResponse};
///
/// let unary: TranscodeResponse = HttpResponse::ok_json(b"{}".to_vec()).into();
/// assert!(matches!(unary, TranscodeResponse::Unary(_)));
/// # }
/// # }
/// ```
#[derive(Debug)]
pub enum TranscodeResponse {
    /// A unary (or otherwise fully buffered) response.
    Unary(HttpResponse),
    /// A server-streaming response with incrementally produced body frames.
    Streaming(StreamingResponse),
}

impl From<HttpResponse> for TranscodeResponse {
    fn from(response: HttpResponse) -> Self {
        Self::Unary(response)
    }
}

impl From<StreamingResponse> for TranscodeResponse {
    fn from(response: StreamingResponse) -> Self {
        Self::Streaming(response)
    }
}

/// A server-streaming failure observed *after* the response headers were sent,
/// carried as an [`http_body::Body`] / stream error so the transport truncates
/// the response.
///
/// Shared by the streaming HTTP adapter and the optional `axum` integration.
#[cfg(any(feature = "serving", feature = "axum"))]
#[derive(Debug)]
pub(crate) struct StreamingError(pub(crate) Status);

#[cfg(any(feature = "serving", feature = "axum"))]
impl fmt::Display for StreamingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "server-streaming response failed: {} ({})", self.0.message(), self.0.code())
    }
}

#[cfg(any(feature = "serving", feature = "axum"))]
impl std::error::Error for StreamingError {}

/// Writes a streaming response's custom `headers` and its negotiated
/// `content_type` onto `dst` (a fresh response header map).
///
/// The negotiated `Content-Type` is authoritative: any custom `content-type`
/// among `headers` is dropped, and the negotiated value is set last. `headers`
/// is consumed so its entries move into `dst` without cloning. A malformed
/// `content_type` (only possible via the caller-supplied
/// [`StreamingResponse::new`]) is dropped rather than panicking, matching
/// [`HttpResponse::into_http`](crate::transcoding::HttpResponse::into_http)'s tolerance.
#[cfg(any(feature = "serving", feature = "axum"))]
pub(crate) fn apply_stream_headers(dst: &mut http::HeaderMap, content_type: &'static str, mut headers: http::HeaderMap) {
    // The negotiated content type is authoritative, so drop any custom one before
    // moving the rest in, then set the negotiated value last. It is built from
    // the static string without copying (`from_str` would allocate per response).
    headers.remove(http::header::CONTENT_TYPE);
    crate::context::append_headers(dst, headers);
    if let Ok(value) = http::HeaderValue::from_maybe_shared(bytes::Bytes::from_static(content_type.as_bytes())) {
        let _ = dst.insert(http::header::CONTENT_TYPE, value);
    }
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    #[test]
    fn streaming_response_debug_is_non_exhaustive() {
        let response = StreamingResponse::encode(stream::iter(vec![Ok::<_, Status>(1_u32)]), StreamEncoding::JsonArray);
        let debug = format!("{response:?}");
        assert!(debug.contains("StreamingResponse"));
        assert!(debug.contains("content_type"));
    }

    #[cfg(any(feature = "serving", feature = "axum"))]
    #[test]
    fn streaming_error_display_carries_message_and_code() {
        let rendered = StreamingError(Status::internal("boom")).to_string();
        assert!(rendered.contains("boom"));
        assert!(rendered.contains("server-streaming response failed"));
    }

    #[cfg(any(feature = "serving", feature = "axum"))]
    #[test]
    fn apply_stream_headers_keeps_content_type_authoritative() {
        let mut custom = http::HeaderMap::new();
        custom.append(http::header::SET_COOKIE, http::HeaderValue::from_static("a=1"));
        custom.append(http::header::SET_COOKIE, http::HeaderValue::from_static("b=2"));
        // A custom `content-type` must be dropped in favour of the negotiated one.
        let _ = custom.insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("text/plain"));

        let mut dst = http::HeaderMap::new();
        apply_stream_headers(&mut dst, "application/json", custom);

        assert_eq!(dst.get_all(http::header::CONTENT_TYPE).iter().count(), 1);
        assert_eq!(dst[http::header::CONTENT_TYPE], "application/json");
        // Repeated custom values are preserved.
        assert_eq!(dst.get_all(http::header::SET_COOKIE).iter().count(), 2);
    }

    #[cfg(any(feature = "serving", feature = "axum"))]
    #[test]
    fn apply_stream_headers_drops_a_malformed_content_type_without_panicking() {
        // A caller-supplied (via `StreamingResponse::new`) invalid content type is
        // dropped rather than panicking in the adapter.
        let mut dst = http::HeaderMap::new();
        apply_stream_headers(&mut dst, "in\nvalid", http::HeaderMap::new());
        assert!(dst.get(http::header::CONTENT_TYPE).is_none());
    }
}
