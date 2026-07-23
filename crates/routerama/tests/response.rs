// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Standalone coverage for the `response` capability.

use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue, LOCATION};
use http::{HeaderMap, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::{
    Body, BoxBody, EitherBodyError, IntoResponse, IntoResponseParts, Response, ResponseParts, StaticBytes, StaticText,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamFailure(&'static str);

impl fmt::Display for StreamFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for StreamFailure {}

#[derive(Debug)]
struct StreamBody {
    frames: VecDeque<Result<Frame<Bytes>, StreamFailure>>,
    size_hint: SizeHint,
}

impl StreamBody {
    fn successful(label: &'static [u8]) -> Self {
        let first = Bytes::from_static(label);
        let second = Bytes::from_static(b":second");
        let mut trailers = HeaderMap::new();
        trailers.insert(HeaderName::from_static("x-stream-complete"), HeaderValue::from_static("yes"));
        Self {
            size_hint: SizeHint::with_exact((first.len() + second.len()) as u64),
            frames: [Ok(Frame::data(first)), Ok(Frame::data(second)), Ok(Frame::trailers(trailers))].into(),
        }
    }

    fn failing(message: &'static str) -> Self {
        Self {
            frames: [Err(StreamFailure(message))].into(),
            size_hint: SizeHint::default(),
        }
    }
}

impl HttpBody for StreamBody {
    type Data = Bytes;
    type Error = StreamFailure;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frames.pop_front())
    }

    fn is_end_stream(&self) -> bool {
        self.frames.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

struct InvalidHeader(StreamBody);

impl IntoResponse for InvalidHeader {
    type Body = StreamBody;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Response::new(self.0);
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-part-rejection"), HeaderValue::from_static("yes"));
        response
    }
}

struct CheckedHeader {
    name: HeaderName,
    value: &'static str,
    rejection: InvalidHeader,
}

impl IntoResponseParts for CheckedHeader {
    type Error = InvalidHeader;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let Self { name, value, rejection } = self;
        let value = value.parse().map_err(|_invalid| rejection)?;
        response.headers_mut().insert(name, value);
        Ok(response)
    }
}

struct LocalBody {
    inner: StreamBody,
    _not_send: Rc<()>,
}

impl HttpBody for LocalBody {
    type Data = Bytes;
    type Error = StreamFailure;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[tokio::test]
async fn built_in_values_and_parts_compose_without_routing() {
    let response = (StatusCode::CREATED, [(LOCATION, HeaderValue::from_static("/books/42"))], "created").into_response();

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()[LOCATION], "/books/42");
    assert_eq!(response.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .expect("built-in response bodies are infallible")
            .to_bytes(),
        b"created"[..]
    );
}

#[tokio::test]
async fn fallible_parts_return_independent_streaming_rejections() {
    let part = CheckedHeader {
        name: HeaderName::from_static("x-checked"),
        value: "contains\nnewline",
        rejection: InvalidHeader(StreamBody::successful(b"invalid-header")),
    };
    let response = (StatusCode::CREATED, part, Response::new(StreamBody::successful(b"discarded"))).into_response();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.headers()["x-part-rejection"], "yes");
    assert!(response.headers().get("x-checked").is_none());

    let mut body = response.into_body();
    assert_eq!(
        body.frame()
            .await
            .expect("the rejection has a first frame")
            .expect("the rejection data frame succeeds")
            .into_data()
            .expect("the first frame contains data"),
        b"invalid-header"[..]
    );
    assert_eq!(
        body.frame()
            .await
            .expect("the rejection has a second frame")
            .expect("the rejection data frame succeeds")
            .into_data()
            .expect("the second frame contains data"),
        b":second"[..]
    );
    let trailers = body
        .frame()
        .await
        .expect("the rejection has trailers")
        .expect("the rejection trailer frame succeeds")
        .into_trailers()
        .expect("the final frame contains trailers");
    assert_eq!(trailers["x-stream-complete"], "yes");
}

#[tokio::test]
async fn concrete_stream_errors_keep_their_branch_and_value() {
    let result: Result<Body, Response<StreamBody>> = Err(Response::new(StreamBody::failing("stream failed")));
    let mut body = result.into_response().into_body();
    let error = body
        .frame()
        .await
        .expect("the failing body yields one frame")
        .expect_err("the frame carries the concrete stream error");

    assert_eq!(error, EitherBodyError::Right(StreamFailure("stream failed")));
}

#[tokio::test]
async fn responses_and_box_body_accept_non_send_bodies() {
    let local_response = Response::new(LocalBody {
        inner: StreamBody::successful(b"direct-local"),
        _not_send: Rc::new(()),
    })
    .into_response();
    let mut direct_body = local_response.into_body();
    assert_eq!(
        direct_body
            .frame()
            .await
            .expect("the direct local body has a data frame")
            .expect("the direct local body data frame succeeds")
            .into_data()
            .expect("the first direct local frame contains data"),
        b"direct-local"[..]
    );

    let mut body = BoxBody::new(LocalBody {
        inner: StreamBody::successful(b"boxed-local"),
        _not_send: Rc::new(()),
    });
    assert_eq!(
        body.frame()
            .await
            .expect("the local body has a data frame")
            .expect("the local body data frame succeeds")
            .into_data()
            .expect("the first local frame contains data"),
        b"boxed-local"[..]
    );

    let mut failing = BoxBody::new(StreamBody::failing("boxed failure"));
    let error = failing
        .frame()
        .await
        .expect("the failing boxed body yields one frame")
        .expect_err("the boxed frame carries an erased error");
    assert_eq!(error.to_string(), "boxed failure");
    assert!(error.as_error().is::<StreamFailure>());
}

#[tokio::test]
async fn text_and_parts_are_composed_without_boxing() {
    let response = (
        StatusCode::CREATED,
        [(LOCATION, HeaderValue::from_static("/books/42"))],
        String::from("created"),
    )
        .into_response();

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()[LOCATION], "/books/42");
    assert_eq!(response.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("all built-in response and part bodies are infallible")
        .to_bytes();
    assert_eq!(body, b"created"[..]);
}

#[tokio::test]
async fn static_responses_retain_the_static_payload_and_metadata() {
    const TEXT: &str = "static text";
    const BYTES: &[u8] = b"\x00static bytes\xff";

    let text_response = StaticText(TEXT).into_response();
    assert_eq!(text_response.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
    let mut text_body = text_response.into_body();
    assert_eq!(text_body.size_hint().exact(), Some(TEXT.len() as u64));
    let text_frame = text_body
        .frame()
        .await
        .expect("a nonempty static text response has one frame")
        .expect("static response frames are infallible")
        .into_data()
        .expect("the static text frame contains data");
    assert_eq!(text_frame.as_ref(), TEXT.as_bytes());
    assert_eq!(text_frame.as_ptr(), TEXT.as_ptr());
    assert!(text_body.frame().await.is_none());

    let bytes_response = StaticBytes(BYTES).into_response();
    assert!(bytes_response.headers().get(CONTENT_TYPE).is_none());
    let mut bytes_body = bytes_response.into_body();
    assert_eq!(bytes_body.size_hint().exact(), Some(BYTES.len() as u64));
    let bytes_frame = bytes_body
        .frame()
        .await
        .expect("a nonempty static byte response has one frame")
        .expect("static response frames are infallible")
        .into_data()
        .expect("the static byte frame contains data");
    assert_eq!(bytes_frame.as_ref(), BYTES);
    assert_eq!(bytes_frame.as_ptr(), BYTES.as_ptr());
    assert!(bytes_body.frame().await.is_none());
}

#[test]
fn response_parts_replace_inner_headers() {
    let response = ([(CONTENT_TYPE, HeaderValue::from_static("application/json"))], String::from("{}")).into_response();

    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(response.headers().get_all(CONTENT_TYPE).iter().count(), 1);
}

#[tokio::test]
async fn result_uses_each_branch_response() {
    let ok: Result<&str, StatusCode> = Ok("yes");
    let error: Result<&str, StatusCode> = Err(StatusCode::CONFLICT);

    let body = ok
        .into_response()
        .into_body()
        .collect()
        .await
        .expect("both built-in response bodies are infallible")
        .to_bytes();
    assert_eq!(body, b"yes"[..]);
    assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
}
