// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapters between generated transcoders and the `http`/`http-body` ecosystem.
//!
//! [`serve_http`] handles one request, [`serve_http_fn`] accepts a closure, and
//! [`RestService`] integrates with `tower` or `layered`. Requests are buffered
//! because JSON decoding requires contiguous bytes. Unary responses are
//! buffered; server-streaming responses forward frames as they arrive.
//! [`RestService::with_max_body_bytes`] adds an incremental request-size limit.
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "tower")] {
//! use rest_over_grpc::serving::RestService;
//! use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
//!
//! // A generated `Transcoder` implements `Transcode`; here it is hand-written.
//! #[derive(Clone)]
//! struct Api;
//! impl Transcode for Api {
//!     fn try_transcode(
//!         &self,
//!         _m: &str,
//!         _t: &str,
//!         _h: http::HeaderMap,
//!         _b: &[u8],
//!     ) -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
//!         async { Some(HttpResponse::ok_json(b"{}".to_vec()).into()) }
//!     }
//! }
//!
//! let _svc = RestService::new(Api).with_max_body_bytes(1 << 20);
//! # }
//! # }
//! ```

#[cfg(feature = "tower")]
use core::convert::Infallible;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::error::Error;

use bytes::{Bytes, BytesMut};
use http::{HeaderMap, Method, Request, Response, StatusCode, Uri};
use http_body::{Body, Frame, SizeHint};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Either, Full, StreamBody};

use crate::transcode_response::{StreamingError, apply_stream_headers};
use crate::transcoding::{HttpResponse, Transcode, TranscodeResponse};

/// A boxed, `Send` body error carried by [`RestBody`]'s server-streaming variant.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// The response body of every serving adapter ([`serve_http`], [`serve_http_fn`],
/// and [`RestService`]).
///
/// A fully-buffered unary body or a live server-streaming frame stream, behind a
/// single [`http_body::Body`] type.
///
/// Unary bodies are stored inline; only streaming bodies are boxed.
pub struct RestBody(RestBodyInner);

type RestBodyInner = Either<Full<Bytes>, UnsyncBoxBody<Bytes, BoxError>>;

impl RestBody {
    /// A fully-buffered response body (the unary path) — no allocation beyond
    /// `bytes`.
    ///
    /// Useful when composing [`serve_http_fn`] with hand-written response arms
    /// that must produce the same [`RestBody`] type (e.g. a custom `404` from a
    /// `hyper` `service_fn`).
    pub fn buffered(bytes: impl Into<Bytes>) -> Self {
        Self(Either::Left(Full::new(bytes.into())))
    }

    /// A live server-streaming frame body.
    fn streaming(body: UnsyncBoxBody<Bytes, BoxError>) -> Self {
        Self(Either::Right(body))
    }
}

impl core::fmt::Debug for RestBody {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RestBody").finish_non_exhaustive()
    }
}

impl Body for RestBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // Both `Either` variants are `Unpin`.
        Pin::new(&mut self.get_mut().0).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.0.size_hint()
    }
}

/// Reads the body of `request` and transcodes it through a closure, returning
/// an [`http::Response`] with a [`RestBody`].
///
/// The closure-taking sibling of [`serve_http`], for a hand-written transcoder
/// rather than a generated one. `transcoder` receives the request method, URI,
/// headers (the request-side gRPC metadata — the `Accept` header among them
/// drives server-streaming content negotiation), and collected body bytes as
/// [`Bytes`], and returns anything convertible into a
/// [`TranscodeResponse`] (so returning a plain [`HttpResponse`] works too). A
/// body that fails to read yields `400 Bad Request` without invoking `transcoder`.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "serving")] {
/// use http_body_util::{BodyExt as _, Full};
/// use rest_over_grpc::serving::serve_http_fn;
/// use rest_over_grpc::transcoding::HttpResponse;
///
/// let request = http::Request::builder()
///     .method(http::Method::GET)
///     .uri("/ok")
///     .body(Full::new(bytes::Bytes::from_static(b"hello")))
///     .expect("valid request");
///
/// let response = futures::executor::block_on(serve_http_fn(
///     request,
///     |_method, _uri, _headers, body| async move { HttpResponse::ok_json(body.to_vec()) },
/// ));
///
/// assert_eq!(response.status(), http::StatusCode::OK);
/// assert_eq!(
///     futures::executor::block_on(response.into_body().collect())
///         .expect("body")
///         .to_bytes()
///         .as_ref(),
///     b"hello",
/// );
/// # }
/// # }
/// ```
pub async fn serve_http_fn<B, D, Fut, R>(request: Request<B>, transcoder: D) -> Response<RestBody>
where
    B: Body,
    D: FnOnce(Method, Uri, HeaderMap, Bytes) -> Fut,
    Fut: Future<Output = R>,
    R: Into<TranscodeResponse>,
{
    let (parts, body) = request.into_parts();
    let Some(bytes) = read_body_uncapped(body).await else {
        return transcode_response_into_http(body_read_failed().into());
    };
    transcode_response_into_http(transcoder(parts.method, parts.uri, parts.headers, bytes).await.into())
}

/// Reads a body in full with no size cap: `Some(bytes)` on success, `None` if the
/// body stream errors. Used by the closure-taking helper, which exposes no size
/// limit (so the capped [`BodyRead::TooLarge`] outcome cannot arise).
async fn read_body_uncapped<B: Body>(body: B) -> Option<Bytes> {
    match body.collect().await {
        // `to_bytes()` yields a contiguous `Bytes`, avoiding a full-body copy.
        Ok(collected) => Some(collected.to_bytes()),
        Err(_) => None,
    }
}

async fn collect_body<B: Body>(body: B, max: Option<usize>) -> BodyRead {
    match max {
        // Uncapped: keep the zero-copy `to_bytes()` fast path.
        None => match read_body_uncapped(body).await {
            Some(bytes) => BodyRead::Ok(bytes),
            None => BodyRead::Failed,
        },
        Some(max) => collect_capped(body, max).await,
    }
}

/// Reads `body` a frame at a time, bailing with [`BodyRead::TooLarge`] as soon
/// as the accumulated length would exceed `max` — so an over-cap upload is
/// rejected without first being buffered in full.
async fn collect_capped<B: Body>(body: B, max: usize) -> BodyRead {
    use bytes::{Buf as _, BufMut as _};

    let mut body = core::pin::pin!(body);
    let mut buf = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return BodyRead::Failed;
        };
        // Only data frames carry body bytes; trailer frames do not count.
        if let Ok(data) = frame.into_data() {
            if buf.len().saturating_add(data.remaining()) > max {
                return BodyRead::TooLarge;
            }
            buf.put(data);
        }
    }
    BodyRead::Ok(buf.freeze())
}

/// The result of reading a request body for the adapters. Read failures are
/// reported distinctly (rather than silently substituting an empty body) so an
/// aborted or truncated upload cannot masquerade as a legitimately empty request,
/// and an over-cap body becomes a `413` rather than a phantom decode.
enum BodyRead {
    /// The full body, read successfully and within the cap (if any).
    Ok(Bytes),
    /// The body exceeded the configured size cap → `413 Payload Too Large`.
    TooLarge,
    /// The body stream produced an error before it finished → `400 Bad Request`.
    Failed,
}

/// The `400 Bad Request` response returned when a request body fails to read
/// (e.g. an aborted or truncated upload).
fn body_read_failed() -> HttpResponse {
    HttpResponse::json(
        StatusCode::BAD_REQUEST,
        br#"{"message":"failed to read the request body"}"#.to_vec(),
    )
}

/// The `413 Payload Too Large` response returned when a body exceeds the cap
/// configured via [`RestService::with_max_body_bytes`].
fn body_too_large() -> HttpResponse {
    HttpResponse::json(
        StatusCode::PAYLOAD_TOO_LARGE,
        br#"{"message":"request body exceeds the configured size limit"}"#.to_vec(),
    )
}

/// The `target` (path plus optional `?query`) a generated transcoder expects,
/// extracted from a request [`Uri`]. Borrowed from the `Uri`, so extracting the
/// target does not allocate.
fn target_of(uri: &Uri) -> &str {
    match uri.path_and_query() {
        Some(path_and_query) => path_and_query.as_str(),
        None => uri.path(),
    }
}

/// Renders a [`TranscodeResponse`] into an [`http::Response`] with a [`RestBody`].
///
/// A unary response becomes a fully-buffered [`RestBody`] with no boxing; a
/// server-streaming response becomes a live frame body that forwards each encoded
/// frame as the handler produces it. A frame-level failure terminates the body
/// with a [`BoxError`], truncating the response (the status line is already sent).
fn transcode_response_into_http(response: TranscodeResponse) -> Response<RestBody> {
    use futures_util::StreamExt as _;

    match response {
        TranscodeResponse::Unary(unary) => {
            let (parts, body) = unary.into_http().into_parts();
            Response::from_parts(parts, RestBody::buffered(Bytes::from(body)))
        }
        TranscodeResponse::Streaming(streaming) => {
            let (content_type, headers, frames) = streaming.into_parts();
            let frames = frames.map(|item| {
                item.map(|bytes| Frame::data(Bytes::from(bytes)))
                    .map_err(|status| Box::new(StreamingError(status)) as BoxError)
            });
            let mut response = Response::new(RestBody::streaming(StreamBody::new(frames).boxed_unsync()));
            apply_stream_headers(response.headers_mut(), content_type, headers);
            response
        }
    }
}

/// Reads the body of `request` and transcodes it through `transcoder`,
/// returning an [`http::Response`] with a [`RestBody`].
///
/// Serves any generated [`Transcode`] directly: it
/// does the [`Uri`]→`target` / [`Method`]→`&str` / `body`→`&[u8]` conversion for
/// you, so a raw handler (a `hyper` `service_fn`, one arm of an ad-hoc router)
/// need not write the wiring closure by hand. Its closure-taking sibling is
/// [`serve_http_fn`]. A body that fails to read yields `400 Bad Request`. This
/// helper imposes no body-size limit; use [`RestService::with_max_body_bytes`]
/// for an opt-in cap.
///
/// The response body is a [`RestBody`] (an [`http_body::Body`]), so the result
/// can be returned straight from an `axum` handler or a `hyper` service, and a
/// server-streaming RPC streams to the wire incrementally.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "serving")] {
/// use http_body_util::{BodyExt as _, Full};
/// use rest_over_grpc::serving::serve_http;
/// use rest_over_grpc::transcoding::Transcode;
/// # use rest_over_grpc::transcoding::{HttpResponse, TranscodeResponse};
/// # struct Echo;
/// # impl Transcode for Echo {
/// #     fn try_transcode(&self, _m: &str, _t: &str, _h: http::HeaderMap, body: &[u8])
/// #         -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
/// #         let body = body.to_vec();
/// #         async move { Some(HttpResponse::ok_json(body).into()) }
/// #     }
/// # }
///
/// let request = http::Request::builder()
///     .uri("/echo")
///     .body(Full::new(bytes::Bytes::from_static(b"hi")))
///     .expect("valid request");
///
/// let response = futures::executor::block_on(serve_http(request, &Echo));
/// assert_eq!(
///     futures::executor::block_on(response.into_body().collect())
///         .expect("body")
///         .to_bytes()
///         .as_ref(),
///     b"hi",
/// );
/// # }
/// # }
/// ```
pub async fn serve_http<B, D>(request: Request<B>, transcoder: &D) -> Response<RestBody>
where
    B: Body,
    D: Transcode,
{
    serve_http_capped(request, transcoder, None).await
}

/// [`serve_http`] with an optional request-body size cap. A `Some(max)` rejects
/// a body longer than `max` bytes with `413 Payload Too Large`, streaming-checked
/// so an over-cap body is never buffered in full; `None` is uncapped. Backs the
/// [`RestService`] cap knob ([`with_max_body_bytes`](RestService::with_max_body_bytes)).
async fn serve_http_capped<B, D>(request: Request<B>, transcoder: &D, max: Option<usize>) -> Response<RestBody>
where
    B: Body,
    D: Transcode,
{
    let (parts, body) = request.into_parts();
    let bytes = match collect_body(body, max).await {
        BodyRead::Ok(bytes) => bytes,
        BodyRead::TooLarge => return transcode_response_into_http(body_too_large().into()),
        BodyRead::Failed => return transcode_response_into_http(body_read_failed().into()),
    };
    let target = target_of(&parts.uri);
    let response = transcoder
        .transcode(parts.method.as_str(), target, parts.headers, bytes.as_ref())
        .await;
    transcode_response_into_http(response)
}

/// A [`tower_service::Service`] / [`layered::Service`] that serves a
/// [`Transcode`] implementation over a web stack.
///
/// Wrap a generated `Transcoder` (or any [`Transcode`])
/// with [`RestService::new`] and mount it: the service reads each request's body
/// and headers, does the [`Uri`]→`target` / [`Method`]→`&str` / `body`→`&[u8]`
/// conversion, calls [`transcode`](crate::transcoding::Transcode::transcode), and
/// returns an [`http::Response`] with a [`RestBody`]. It handles both unary and
/// server-streaming RPCs — a unary reply is buffered, a server-streaming reply is
/// forwarded frame by frame. It implements both [`tower_service::Service`]
/// (feature `tower`) and [`layered::Service`] (feature `layered`).
///
/// A shared transcoder works too: [`Arc<T>`](std::sync::Arc) implements
/// [`Transcode`], so `RestService::new(Arc::new(transcoder))`
/// is cheap to clone into the stack.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "tower")] {
/// use rest_over_grpc::serving::RestService;
/// # use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
/// # #[derive(Clone)] struct Api;
/// # impl Transcode for Api {
/// #     fn try_transcode(&self, _m: &str, _t: &str, _h: http::HeaderMap, _b: &[u8])
/// #         -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
/// #         async { Some(HttpResponse::ok_json(b"{}".to_vec()).into()) }
/// #     }
/// # }
///
/// // `Api` is a generated `Transcoder`; cap buffered requests when serving
/// // untrusted clients.
/// let _service = RestService::new(Api).with_max_body_bytes(1 << 20);
/// # }
/// # }
/// ```
#[cfg(any(feature = "tower", feature = "layered"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "tower", feature = "layered"))))]
#[derive(Debug, Clone)]
pub struct RestService<T> {
    transcoder: T,
    max_body_bytes: Option<usize>,
}

#[cfg(any(feature = "tower", feature = "layered"))]
impl<T> RestService<T> {
    /// Wraps a [`Transcode`] implementation
    /// (typically a generated `Transcoder`) as a service.
    ///
    /// The request body is buffered without a size cap; call
    /// [`with_max_body_bytes`](Self::with_max_body_bytes) to bound it.
    pub const fn new(transcoder: T) -> Self {
        Self {
            transcoder,
            max_body_bytes: None,
        }
    }

    /// Caps the request body at `max` bytes: a longer body is rejected with
    /// `413 Payload Too Large` before it is fully buffered (the length is checked
    /// as the body streams in, so an over-cap upload cannot exhaust memory).
    ///
    /// Uncapped by default, matching the neutral, policy-free contract of the
    /// free [`serve_http`] helper. For finer control (an HTTP-level `415`, a
    /// custom over-limit body), read the body yourself and call the generated
    /// `transcode` directly, as the `custom_body_handling` example shows. This
    /// bounds only the buffered request body; it does not cap a server-streaming
    /// *response*, which is forwarded frame by frame.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "tower")] {
    /// use rest_over_grpc::serving::RestService;
    /// # use rest_over_grpc::transcoding::{HttpResponse, Transcode, TranscodeResponse};
    /// # #[derive(Clone)] struct Api;
    /// # impl Transcode for Api {
    /// #     fn try_transcode(&self, _m: &str, _t: &str, _h: http::HeaderMap, _b: &[u8])
    /// #         -> impl core::future::Future<Output = Option<TranscodeResponse>> + Send {
    /// #         async { Some(HttpResponse::ok_json(b"{}".to_vec()).into()) }
    /// #     }
    /// # }
    /// let _service = RestService::new(Api).with_max_body_bytes(1 << 20);
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub const fn with_max_body_bytes(mut self, max: usize) -> Self {
        self.max_body_bytes = Some(max);
        self
    }
}

#[cfg(feature = "tower")]
impl<B, T> tower_service::Service<Request<B>> for RestService<T>
where
    B: Body + Send + 'static,
    B::Data: Send,
    T: Transcode + Clone + Send + 'static,
{
    type Response = Response<RestBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    #[cfg_attr(test, mutants::skip)]
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let transcoder = self.transcoder.clone();
        let max = self.max_body_bytes;
        Box::pin(async move { Ok(serve_http_capped(req, &transcoder, max).await) })
    }
}

/// Serves a [`Transcode`] as a [`layered::Service`].
#[cfg(feature = "layered")]
impl<B, T> layered::Service<Request<B>> for RestService<T>
where
    B: Body + Send + 'static,
    B::Data: Send,
    T: Transcode + Send + Sync,
{
    type Out = Response<RestBody>;

    fn execute(&self, input: Request<B>) -> impl Future<Output = Self::Out> + Send {
        serve_http_capped(input, &self.transcoder, self.max_body_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_of_falls_back_to_path_when_there_is_no_query() {
        let uri: Uri = "example.com:443".parse().expect("authority uri");
        assert_eq!(uri.path_and_query(), None);
        assert_eq!(target_of(&uri), uri.path());
    }

    #[test]
    fn target_of_returns_path_and_query_when_present() {
        let uri: Uri = "/v1/x?a=1".parse().expect("origin-form uri");
        assert_eq!(target_of(&uri), "/v1/x?a=1");
    }

    #[test]
    fn rest_body_reports_size_hint_end_stream_and_debug() {
        let body = RestBody::buffered(Bytes::from_static(b"hello"));
        assert_eq!(body.size_hint().exact(), Some(5));
        assert!(!body.is_end_stream());
        assert!(format!("{body:?}").contains("RestBody"));

        let empty = RestBody::buffered(Bytes::new());
        assert_eq!(empty.size_hint().exact(), Some(0));
        assert!(empty.is_end_stream());
    }
}
