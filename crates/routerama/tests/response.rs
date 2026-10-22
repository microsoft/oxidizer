// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Standalone coverage for the `response` capability.

use std::collections::VecDeque;
use std::error::Error as _;
use std::fmt;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HeaderName, HeaderValue, LOCATION};
use http::{HeaderMap, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::{
    Body, BoxBody, DataEitherBody, EitherBodyError, HeterogeneousResult, IntoResponse, IntoResponseParts, Response, ResponseParts,
    SendBoxBody, StaticBytes, StaticText,
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
    pending_once: bool,
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
            pending_once: false,
        }
    }

    fn failing(message: &'static str) -> Self {
        Self {
            frames: [Err(StreamFailure(message))].into(),
            size_hint: SizeHint::default(),
            pending_once: false,
        }
    }

    fn empty() -> Self {
        Self {
            frames: VecDeque::new(),
            size_hint: SizeHint::with_exact(0),
            pending_once: false,
        }
    }

    /// Returns Pending exactly once before yielding the wrapped frames.
    fn pending_once(mut self) -> Self {
        self.pending_once = true;
        self
    }
}

impl HttpBody for StreamBody {
    type Data = Bytes;
    type Error = StreamFailure;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.pending_once {
            self.pending_once = false;
            return Poll::Pending;
        }
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

/// Wraps a [`StreamBody`] so it can appear on either branch of a `Result` or
/// [`HeterogeneousResult`] response conversion, letting tests drive the
/// underlying stream's pending, error, and end-of-stream cases through
/// [`EitherBody`](routerama::response::EitherBody) and [`DataEitherBody`].
struct StreamResponse(StreamBody);

impl IntoResponse for StreamResponse {
    type Body = StreamBody;

    fn into_response(self) -> Response<Self::Body> {
        Response::new(self.0)
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

#[test]
fn body_into_bytes_and_unit_conversion_round_trip() {
    assert_eq!(Body::empty().into_bytes(), Bytes::new());

    let payload = Bytes::from_static(b"payload");
    assert_eq!(Body::from_bytes(payload.clone()).into_bytes(), payload);

    // `Body::from(())` and the unit `IntoResponse` impl both produce an empty body.
    assert_eq!(Body::from(()).into_bytes(), Bytes::new());
    let unit_response = ().into_response();
    assert_eq!(unit_response.into_body().into_bytes(), Bytes::new());
}

#[tokio::test]
async fn either_body_reports_pending_from_both_branches_before_delegating() {
    let ok: Result<StreamResponse, StreamResponse> = Ok(StreamResponse(StreamBody::successful(b"left").pending_once()));
    let mut left_body = Box::pin(ok.into_response().into_body());
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(left_body.as_mut().poll_frame(&mut context), Poll::Pending));
    let frame = left_body
        .as_mut()
        .poll_frame(&mut context)
        .map(|option| option.expect("the left branch yields a frame after pending"));
    let Poll::Ready(Ok(frame)) = frame else {
        panic!("the left branch resolves to a successful data frame");
    };
    assert_eq!(frame.into_data().expect("the frame contains data"), b"left"[..]);

    let error: Result<StreamResponse, StreamResponse> = Err(StreamResponse(StreamBody::successful(b"right").pending_once()));
    let mut right_body = Box::pin(error.into_response().into_body());
    assert!(matches!(right_body.as_mut().poll_frame(&mut context), Poll::Pending));
    let frame = right_body
        .as_mut()
        .poll_frame(&mut context)
        .map(|option| option.expect("the right branch yields a frame after pending"));
    let Poll::Ready(Ok(frame)) = frame else {
        panic!("the right branch resolves to a successful data frame");
    };
    assert_eq!(frame.into_data().expect("the frame contains data"), b"right"[..]);
}

#[tokio::test]
async fn either_body_error_display_and_source_report_each_failing_branch() {
    let ok_failure: Result<StreamResponse, StreamResponse> = Ok(StreamResponse(StreamBody::failing("left-fail")));
    let mut left_body = Box::pin(ok_failure.into_response().into_body());
    let mut context = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(Err(error))) = left_body.as_mut().poll_frame(&mut context) else {
        panic!("the left branch immediately fails");
    };
    assert_eq!(error.to_string(), "left-fail");
    assert!(error.source().is_some());
    assert_eq!(error, EitherBodyError::Left(StreamFailure("left-fail")));

    let err_failure: Result<StreamResponse, StreamResponse> = Err(StreamResponse(StreamBody::failing("right-fail")));
    let mut right_body = Box::pin(err_failure.into_response().into_body());
    let Poll::Ready(Some(Err(error))) = right_body.as_mut().poll_frame(&mut context) else {
        panic!("the right branch immediately fails");
    };
    assert_eq!(error.to_string(), "right-fail");
    assert!(error.source().is_some());
    assert_eq!(error, EitherBodyError::Right(StreamFailure("right-fail")));
}

#[tokio::test]
async fn data_either_body_forwards_every_branch_outcome_and_inspection() {
    // Left success, drained to `None`, with direct `is_end_stream`/`size_hint` inspection.
    let ok_success: HeterogeneousResult<StreamResponse, StreamResponse> = Ok(StreamResponse(StreamBody::successful(b"left"))).into();
    let left_body = ok_success.into_response().into_body();
    assert!(!left_body.is_end_stream());
    assert_eq!(left_body.size_hint().exact(), Some(11));
    let mut left_body = Box::pin(left_body);
    let mut context = Context::from_waker(Waker::noop());
    while let Poll::Ready(Some(Ok(_))) = left_body.as_mut().poll_frame(&mut context) {}
    assert!(left_body.is_end_stream());

    // Left pending, then failing.
    let ok_pending_failure: HeterogeneousResult<StreamResponse, StreamResponse> =
        Ok(StreamResponse(StreamBody::failing("left-data-fail").pending_once())).into();
    let mut left_pending = Box::pin(ok_pending_failure.into_response().into_body());
    assert!(matches!(left_pending.as_mut().poll_frame(&mut context), Poll::Pending));
    let Poll::Ready(Some(Err(error))) = left_pending.as_mut().poll_frame(&mut context) else {
        panic!("the left branch fails after pending");
    };
    assert_eq!(error.to_string(), "left-data-fail");

    // Right failure, drained-to-`None`, pending, and direct inspection.
    let err_failure: HeterogeneousResult<StreamResponse, StreamResponse> =
        Err(StreamResponse(StreamBody::failing("right-data-fail"))).into();
    let mut right_body = Box::pin(err_failure.into_response().into_body());
    let Poll::Ready(Some(Err(error))) = right_body.as_mut().poll_frame(&mut context) else {
        panic!("the right branch fails immediately");
    };
    assert_eq!(error.to_string(), "right-data-fail");

    let err_empty: HeterogeneousResult<StreamResponse, StreamResponse> = Err(StreamResponse(StreamBody::empty())).into();
    let right_empty: DataEitherBody<<StreamResponse as IntoResponse>::Body, <StreamResponse as IntoResponse>::Body> =
        err_empty.into_response().into_body();
    assert!(right_empty.is_end_stream());
    assert_eq!(right_empty.size_hint().exact(), Some(0));
    let mut right_empty = Box::pin(right_empty);
    assert!(matches!(right_empty.as_mut().poll_frame(&mut context), Poll::Ready(None)));

    let err_pending: HeterogeneousResult<StreamResponse, StreamResponse> =
        Err(StreamResponse(StreamBody::successful(b"right-pending").pending_once())).into();
    let mut right_pending = Box::pin(err_pending.into_response().into_body());
    assert!(matches!(right_pending.as_mut().poll_frame(&mut context), Poll::Pending));
}

#[tokio::test]
async fn box_body_forwards_pending_and_debug_formats_without_disclosing_storage() {
    let mut body = Box::pin(BoxBody::new(StreamBody::successful(b"boxed").pending_once()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(body.as_mut().poll_frame(&mut context), Poll::Pending));
    assert_eq!(
        body.as_mut()
            .frame()
            .await
            .expect("the boxed body yields a frame after pending")
            .expect("the boxed frame succeeds")
            .into_data()
            .expect("the frame contains data"),
        b"boxed"[..]
    );

    let debug_body = BoxBody::new(StreamBody::empty());
    assert_eq!(format!("{debug_body:?}"), "BoxBody { .. }");
}

#[tokio::test]
async fn box_body_error_exposes_its_inner_error_by_reference_and_by_value() {
    let mut failing = BoxBody::new(StreamBody::failing("boxed-inner"));
    let error = failing
        .frame()
        .await
        .expect("the failing boxed body yields one frame")
        .expect_err("the boxed frame carries an erased error");
    assert_eq!(error.to_string(), "boxed-inner");
    assert!(error.source().is_some());
    let inner = error.into_inner();
    assert_eq!(inner.to_string(), "boxed-inner");
}

#[tokio::test]
async fn send_box_body_forwards_pending_debug_size_hint_and_boxes_errors() {
    let mut body = Box::pin(SendBoxBody::new(StreamBody::successful(b"sent").pending_once()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(body.as_mut().poll_frame(&mut context), Poll::Pending));
    assert_eq!(
        body.as_mut()
            .frame()
            .await
            .expect("the send body yields a frame after pending")
            .expect("the send frame succeeds")
            .into_data()
            .expect("the frame contains data"),
        b"sent"[..]
    );

    let debug_body = SendBoxBody::new(StreamBody::empty());
    assert_eq!(format!("{debug_body:?}"), "SendBoxBody { .. }");
    assert_eq!(debug_body.size_hint().exact(), Some(0));

    let mut failing = SendBoxBody::new(StreamBody::failing("send-inner"));
    let error = failing
        .frame()
        .await
        .expect("the failing send body yields one frame")
        .expect_err("the send frame carries an erased error");
    assert_eq!(error.to_string(), "send-inner");
    assert!(error.source().is_some());
    assert_eq!(error.as_error().to_string(), "send-inner");
    let inner = error.into_inner();
    assert_eq!(inner.to_string(), "send-inner");
}

#[test]
fn json_number_slots_cover_every_signed_and_unsigned_integer_primitive() {
    use routerama::response::json_body_template;

    let body = json_body_template!(
        a = number(1_u8),
        b = number(2_u16),
        c = number(3_u32),
        d = number(4_u64),
        e = number(5_i8),
        f = number(-6_i16),
        g = number(7_i32),
        h = number(-8_i64),
        i = number(9_usize),
        j = number(-10_isize);
        "[", a, ",", b, ",", c, ",", d, ",", e, ",", f, ",", g, ",", h, ",", i, ",", j, "]"
    );

    assert_eq!(body.as_bytes(), b"[1,2,3,4,5,-6,7,-8,9,-10]");
}
