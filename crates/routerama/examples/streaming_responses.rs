// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Streaming response bodies: data frames, trailers, and body errors.
//!
//! Run with
//! `cargo run -p routerama --example streaming_responses --features route`.
//!
//! A handler's response body is whatever concrete `http_body::Body<Data =
//! Bytes>` its `IntoResponse` conversion retains. The macro collects the
//! finite set of body types a service can return into one private sum and
//! forwards `poll_frame` straight to the active variant, so a streaming body
//! keeps its own frames, trailers, end-of-stream state, size hint, and error
//! type. Nothing is buffered, boxed, or required to be `Send`.
//!
//! This example streams three kinds of response from one router:
//!
//! - a chunked body that ends with an HTTP trailer field;
//! - a body that fails partway through, so the transport sees earlier frames
//!   followed by a body error; and
//! - a `Result` whose two arms have different body types and therefore travel
//!   in an unboxed [`EitherBody`].
//!
//! Because a streaming body's status and headers are sent before the first
//! frame is polled, a mid-stream failure cannot change the status. That is a
//! property of HTTP, not of Routerama, and the assertions below show it.
//!
//! The generated body sum owns each handler body's concrete error value and
//! reports *which* response failed; it deliberately does not re-`Display` the
//! inner message or expose it through `Error::source`, so a service's private
//! error types cannot leak through the opaque return type.
//!
//! [`EitherBody`]: routerama::response::EitherBody

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::VecDeque;

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};
use http_body::{Body as HttpBody, Frame, SizeHint};
use routerama::response::{IntoResponse, Response};
use routerama::route::{HeaderMap, Request, StatusCode, router};

/// The concrete error type of [`Chunks`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChunkFailure(&'static str);

impl fmt::Display for ChunkFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl core::error::Error for ChunkFailure {}

/// A scripted streaming body: data frames, then an optional trailer or error.
#[derive(Debug)]
struct Chunks {
    frames: VecDeque<Result<Frame<Bytes>, ChunkFailure>>,
}

impl Chunks {
    /// Streams `parts` and finishes with an `x-chunks` trailer field.
    fn with_trailers(parts: &[&'static str]) -> Self {
        let mut trailers = HeaderMap::new();
        _ = trailers.insert(
            HeaderName::from_static("x-chunks"),
            HeaderValue::from_str(&parts.len().to_string()).expect("a decimal count is a valid header value"),
        );
        let mut frames: VecDeque<_> = parts
            .iter()
            .map(|part| Ok(Frame::data(Bytes::from_static(part.as_bytes()))))
            .collect();
        frames.push_back(Ok(Frame::trailers(trailers)));
        Self { frames }
    }

    /// Streams `parts` and then fails.
    fn failing_after(parts: &[&'static str], message: &'static str) -> Self {
        let mut frames: VecDeque<_> = parts
            .iter()
            .map(|part| Ok(Frame::data(Bytes::from_static(part.as_bytes()))))
            .collect();
        frames.push_back(Err(ChunkFailure(message)));
        Self { frames }
    }
}

impl HttpBody for Chunks {
    type Data = Bytes;
    type Error = ChunkFailure;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frames.pop_front())
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

impl IntoResponse for Chunks {
    type Body = Self;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Response::new(self);
        _ = response
            .headers_mut()
            .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        _ = response
            .headers_mut()
            .insert(http::header::TRAILER, HeaderValue::from_static("x-chunks"));
        response
    }
}

struct Feed;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Feed {
    /// Streams data frames and announces a trailer field.
    #[route(GET, "/feed")]
    async fn feed(&self) -> Chunks {
        Chunks::with_trailers(&["first ", "second ", "third"])
    }

    /// Streams two frames and then fails with its own error type.
    #[route(GET, "/feed/broken")]
    async fn broken(&self) -> Chunks {
        Chunks::failing_after(&["partial ", "output"], "upstream disconnected")
    }

    /// Returns one of two different body types without boxing either.
    #[route(GET, "/feed/{name}")]
    async fn named(&self, name: &str) -> Result<Chunks, (StatusCode, String)> {
        if name == "known" {
            Ok(Chunks::with_trailers(&["known"]))
        } else {
            Err((StatusCode::NOT_FOUND, format!("no feed named {name}")))
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let response = dispatch("/feed").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let streamed = drain(response).await;
    assert_eq!(streamed.data, b"first second third"[..]);
    assert_eq!(streamed.trailers.expect("the trailer frame is forwarded")["x-chunks"], "3");
    assert_eq!(streamed.error, None);

    // Frames produced before the failure still reach the transport. The error
    // the transport observes is the generated sum's, which names the failing
    // response rather than repeating the handler's private message.
    let response = dispatch("/feed/broken").await;
    assert_eq!(response.status(), StatusCode::OK);
    let streamed = drain(response).await;
    assert_eq!(streamed.data, b"partial output"[..]);
    assert_eq!(streamed.trailers, None);
    assert_eq!(
        streamed.error.as_deref(),
        Some("response body from handler response `Chunks` failed")
    );

    // `Result` keeps both arms concrete inside an `EitherBody`.
    let streamed = drain(dispatch("/feed/known").await).await;
    assert_eq!(streamed.data, b"known"[..]);

    let response = dispatch("/feed/other").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(drain(response).await.data, b"no feed named other"[..]);
}

async fn dispatch(path: &'static str) -> Response<impl HttpBody<Data = Bytes, Error: fmt::Display>> {
    let request = Request::get(path).body(()).expect("static request metadata is valid");
    Feed.route(request, &()).await
}

/// Everything a transport observes while polling a response body to the end.
struct Streamed {
    data: Bytes,
    trailers: Option<HeaderMap>,
    error: Option<String>,
}

async fn drain<B>(response: Response<B>) -> Streamed
where
    B: HttpBody<Data = Bytes>,
    B::Error: fmt::Display,
{
    let mut body = core::pin::pin!(response.into_body());
    let mut data = bytes::BytesMut::new();
    let mut trailers = None;
    let mut error = None;

    while let Some(frame) = core::future::poll_fn(|context| body.as_mut().poll_frame(context)).await {
        match frame {
            Ok(frame) => match frame.into_data() {
                Ok(chunk) => data.extend_from_slice(&chunk),
                Err(frame) => trailers = frame.into_trailers().ok(),
            },
            Err(failure) => {
                error = Some(failure.to_string());
                break;
            }
        }
    }

    Streamed {
        data: data.freeze(),
        trailers,
        error,
    }
}
