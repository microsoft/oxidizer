// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for generated heterogeneous response-body sums.

#![deny(private_bounds, private_interfaces)]
#![forbid(unsafe_code)]

use std::cell::Cell;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::fmt;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, StatusCode, Version};
use http_body::{Body as _, Frame, SizeHint};
use routerama::response::{
    Body, BoxBody, EitherBody, EitherBodyError, IntoResponse, IntoResponseParts, NeverBody, Response, ResponseParts,
};
use routerama::route::{FromRequestParts, Request, router};

/// A concrete error from the test's public streaming body example.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamFailure(&'static str);

impl fmt::Display for StreamFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for StreamFailure {}

/// A public concrete streaming body used by custom response and rejection examples.
#[derive(Debug)]
pub struct StreamBody {
    frames: VecDeque<Result<Frame<Bytes>, StreamFailure>>,
    size_hint: SizeHint,
}

impl StreamBody {
    fn success(label: &'static [u8]) -> Self {
        let mut trailers = HeaderMap::new();
        trailers.insert(HeaderName::from_static("x-stream-complete"), HeaderValue::from_static("yes"));
        let first = Bytes::from_static(label);
        let second = Bytes::from_static(b":second");
        Self {
            size_hint: SizeHint::with_exact((first.len() + second.len()) as u64),
            frames: [Ok(Frame::data(first)), Ok(Frame::data(second)), Ok(Frame::trailers(trailers))].into(),
        }
    }

    fn failed(message: &'static str) -> Self {
        Self {
            frames: [Err(StreamFailure(message))].into(),
            size_hint: SizeHint::default(),
        }
    }
}

impl http_body::Body for StreamBody {
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

/// A streaming rejection produced by [`CheckedHeader`].
#[derive(Debug)]
pub struct StreamingPartsRejection {
    body: StreamBody,
}

impl StreamingPartsRejection {
    /// Creates a rejection with data frames and trailers.
    #[must_use]
    pub fn streaming(label: &'static [u8]) -> Self {
        Self {
            body: StreamBody::success(label),
        }
    }

    /// Creates a rejection whose concrete response body fails while streaming.
    #[must_use]
    pub fn failing(message: &'static str) -> Self {
        Self {
            body: StreamBody::failed(message),
        }
    }
}

impl IntoResponse for StreamingPartsRejection {
    type Body = StreamBody;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Response::new(self.body);
        *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-part-rejection"), HeaderValue::from_static("yes"));
        response
    }
}

/// Parses and inserts one response header, rejecting invalid values.
///
/// This is a public custom-part example in the integration-test crate rather
/// than a permanent Routerama helper. Its rejection streams a concrete body.
#[derive(Debug)]
pub struct CheckedHeader {
    name: HeaderName,
    value: &'static str,
    rejection: StreamingPartsRejection,
}

impl CheckedHeader {
    /// Creates a checked header and its typed rejection.
    #[must_use]
    pub const fn new(name: HeaderName, value: &'static str, rejection: StreamingPartsRejection) -> Self {
        Self { name, value, rejection }
    }
}

impl IntoResponseParts for CheckedHeader {
    type Error = StreamingPartsRejection;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let value = self.value.parse().map_err(|_invalid_header| self.rejection)?;
        response.headers_mut().insert(self.name, value);
        Ok(response)
    }
}

struct ObservedCheckedHeader<'a> {
    part: CheckedHeader,
    calls: &'a Cell<u8>,
    id: u8,
}

impl IntoResponseParts for ObservedCheckedHeader<'_> {
    type Error = StreamingPartsRejection;

    fn into_response_parts(self, response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        self.calls.set(self.calls.get() * 10 + self.id);
        self.part.into_response_parts(response)
    }
}

struct AlwaysReject;

struct StreamingRejection;

impl<S: ?Sized> FromRequestParts<'_, S> for AlwaysReject {
    type Rejection = StreamingRejection;

    fn from_request_parts(_parts: &http::request::Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Err(StreamingRejection)
    }
}

impl IntoResponse for StreamingRejection {
    type Body = StreamBody;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Response::new(StreamBody::success(b"rejected"));
        *response.status_mut() = StatusCode::IM_A_TEAPOT;
        response
    }
}

struct LocalBody {
    body: StreamBody,
    _not_send: Rc<()>,
}

#[derive(Debug)]
struct LocalFailure {
    _not_send: Rc<()>,
}

impl fmt::Display for LocalFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("local body failure")
    }
}

impl core::error::Error for LocalFailure {}

impl http_body::Body for LocalBody {
    type Data = Bytes;
    type Error = LocalFailure;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.body).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
            Poll::Ready(Some(Err(_))) => Poll::Ready(Some(Err(LocalFailure { _not_send: Rc::new(()) }))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// Public service whose private response body types stay behind the opaque API.
#[derive(Debug)]
pub struct ResponseBodies;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "the test deliberately proves that Routerama accepts a local handler future; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl ResponseBodies {
    #[route(GET, "/fixed")]
    async fn fixed(&self) -> Bytes {
        Bytes::from_static(b"fixed")
    }

    #[route(GET, "/stream")]
    async fn stream(&self) -> (StatusCode, [(HeaderName, HeaderValue); 1], Response<StreamBody>) {
        (
            StatusCode::PARTIAL_CONTENT,
            [(HeaderName::from_static("x-stream"), HeaderValue::from_static("direct"))],
            Response::new(StreamBody::success(b"first")),
        )
    }

    #[route(GET, "/result/{value}")]
    async fn result(&self, value: u8) -> Result<Response<StreamBody>, (StatusCode, &'static str)> {
        if value == 1 {
            Ok(Response::new(StreamBody::success(b"result")))
        } else {
            Err((StatusCode::CONFLICT, "result-error"))
        }
    }

    #[route(GET, "/rejected")]
    async fn rejected(&self, _rejection: AlwaysReject) -> Bytes {
        unreachable!("the rejecting extractor must short-circuit this handler")
    }

    #[route(GET, "/boxed")]
    async fn boxed(&self) -> BoxBody {
        BoxBody::new(StreamBody::success(b"boxed"))
    }

    #[route(GET, "/failure")]
    async fn failure(&self) -> Response<StreamBody> {
        Response::new(StreamBody::failed("stream failed"))
    }

    #[route(GET, "/fallible-parts/{mode}")]
    async fn fallible_parts(&self, mode: u8) -> (CheckedHeader, Response<StreamBody>) {
        let (value, rejection, body) = match mode {
            0 => (
                "ready",
                StreamingPartsRejection::streaming(b"unused"),
                StreamBody::success(b"handler"),
            ),
            1 => (
                "contains\nnewline",
                StreamingPartsRejection::streaming(b"part-rejected"),
                StreamBody::success(b"discarded"),
            ),
            2 => (
                "contains\nnewline",
                StreamingPartsRejection::failing("part rejection stream failed"),
                StreamBody::success(b"discarded"),
            ),
            _ => (
                "ready",
                StreamingPartsRejection::streaming(b"unused"),
                StreamBody::failed("handler tuple stream failed"),
            ),
        };
        (
            CheckedHeader::new(HeaderName::from_static("x-checked"), value, rejection),
            Response::new(body),
        )
    }

    #[route(GET, "/local")]
    async fn local(&self) -> Response<LocalBody> {
        let not_send = Rc::new(());
        core::future::ready(()).await;
        Response::new(LocalBody {
            body: StreamBody::success(b"local"),
            _not_send: not_send,
        })
    }

    #[route(dynamic)]
    async fn dynamic_stream(&self, #[capture] name: String) -> Response<StreamBody> {
        let first = Bytes::from(name);
        let second = Bytes::from_static(b":dynamic");
        Response::new(StreamBody {
            size_hint: SizeHint::with_exact((first.len() + second.len()) as u64),
            frames: [Ok(Frame::data(first)), Ok(Frame::data(second))].into(),
        })
    }
}

/// Public static service that returns a private streaming body type.
#[derive(Debug)]
pub struct PublicStaticBodies;

struct PublicStaticBodiesResponseBody;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PublicStaticBodies {
    #[route(GET, "/stream")]
    async fn stream(&self) -> Response<StreamBody> {
        Response::new(StreamBody::success(b"transport"))
    }
}

#[tokio::test]
async fn fixed_and_streaming_bodies_share_one_generated_service_body() {
    let router = response_router();
    let service = ResponseBodies;

    let fixed = router.route(&service, request("/fixed"), &()).await;
    let fixed = observe(fixed.into_body()).await;
    assert_eq!(fixed.chunks, [Bytes::from_static(b"fixed")]);
    assert!(fixed.trailers.is_none());

    let streaming = router.route(&service, request("/stream"), &()).await;
    assert_eq!(streaming.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(streaming.headers()["x-stream"], "direct");
    assert_eq!(streaming.body().size_hint().exact(), Some(12));
    assert!(!streaming.body().is_end_stream());
    let streaming = observe(streaming.into_body()).await;
    assert_eq!(streaming.chunks, [Bytes::from_static(b"first"), Bytes::from_static(b":second")]);
    assert_eq!(
        streaming.trailers.expect("the trailer frame is preserved")["x-stream-complete"],
        "yes"
    );
    assert!(streaming.ended);
}

#[tokio::test]
async fn result_and_streaming_rejection_branches_retain_their_bodies() {
    let router = response_router();
    let service = ResponseBodies;

    let success = router.route(&service, request("/result/1"), &()).await;
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(observe(success.into_body()).await.chunks[0], "result");

    let error = router.route(&service, request("/result/2"), &()).await;
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(observe(error.into_body()).await.chunks, [Bytes::from_static(b"result-error")]);

    let rejected = router.route(&service, request("/rejected"), &()).await;
    assert_eq!(rejected.status(), StatusCode::IM_A_TEAPOT);
    let rejected = observe(rejected.into_body()).await;
    assert_eq!(rejected.chunks[0], "rejected");
    assert_eq!(
        rejected.trailers.expect("rejection trailers are preserved")["x-stream-complete"],
        "yes"
    );
}

#[tokio::test]
async fn existing_parts_remain_infallible_and_leftmost_duplicates_win() {
    fn assert_infallible<P: IntoResponseParts<Error = Infallible>>() {}

    assert_infallible::<StatusCode>();
    assert_infallible::<HeaderMap>();
    assert_infallible::<[(HeaderName, HeaderValue); 1]>();
    assert_eq!(core::mem::size_of::<NeverBody>(), 0);
    assert_eq!(
        core::mem::size_of::<<(StatusCode, String) as IntoResponse>::Body>(),
        core::mem::size_of::<Body>()
    );

    let mut status_body = Response::new(Body::from("status"));
    *status_body.status_mut() = StatusCode::ACCEPTED;
    let status = (StatusCode::IM_A_TEAPOT, StatusCode::CREATED, status_body).into_response();
    assert_eq!(status.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(observe(status.into_body()).await.chunks, [Bytes::from_static(b"status")]);

    let duplicate = HeaderName::from_static("x-duplicate");
    let mut header_body = Response::new(Body::from("headers"));
    header_body
        .headers_mut()
        .insert(duplicate.clone(), HeaderValue::from_static("inner"));
    let headers = (
        [(duplicate.clone(), HeaderValue::from_static("left"))],
        [(duplicate.clone(), HeaderValue::from_static("right"))],
        header_body,
    )
        .into_response();
    assert_eq!(headers.headers()[duplicate], "left");
    assert_eq!(headers.headers().get_all("x-duplicate").iter().count(), 1);

    let mut left = HeaderMap::new();
    left.insert("x-map-duplicate", HeaderValue::from_static("left"));
    let mut right = HeaderMap::new();
    right.insert("x-map-duplicate", HeaderValue::from_static("right"));
    let maps = (left, right, Body::empty()).into_response();
    assert_eq!(maps.headers()["x-map-duplicate"], "left");
    assert_eq!(maps.headers().get_all("x-map-duplicate").iter().count(), 1);
}

#[test]
fn custom_parts_can_inspect_and_modify_all_response_metadata() {
    struct InspectMetadata;

    impl IntoResponseParts for InspectMetadata {
        type Error = Infallible;

        fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
            assert_eq!(response.status(), StatusCode::CREATED);
            assert_eq!(response.version(), Version::HTTP_11);
            assert!(response.headers().is_empty());
            assert!(response.extensions().get::<u32>().is_none());
            assert!(format!("{response:?}").contains("ResponseParts"));

            *response.version_mut() = Version::HTTP_2;
            response.extensions_mut().insert(42_u32);
            Ok(response)
        }
    }

    let response = (InspectMetadata, StatusCode::CREATED, Body::empty()).into_response();

    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.extensions().get::<u32>(), Some(&42));
}

#[tokio::test]
async fn first_part_failure_discards_success_metadata_and_returns_its_rejection() {
    let calls = Cell::new(0);
    let first = observed_checked_header(&calls, 1, "x-first", "contains\nnewline", b"first-failed");
    let second = observed_checked_header(&calls, 2, "x-second", "applied", b"unused");

    let response: Response<EitherBody<Body, EitherBody<StreamBody, StreamBody>>> = (first, second, Body::from("discarded")).into_response();

    assert_eq!(calls.get(), 21);
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.headers()["x-part-rejection"], "yes");
    assert!(
        response.headers().get("x-second").is_none(),
        "metadata applied to the discarded success response must not escape"
    );
    let rejection = observe(response.into_body()).await;
    assert_eq!(
        rejection.chunks,
        [Bytes::from_static(b"first-failed"), Bytes::from_static(b":second")]
    );
    assert_eq!(
        rejection.trailers.expect("streaming part rejection retains trailers")["x-stream-complete"],
        "yes"
    );
}

#[tokio::test]
async fn second_part_failure_short_circuits_the_first_part() {
    let calls = Cell::new(0);
    let first = observed_checked_header(&calls, 1, "x-first", "would-apply", b"unused");
    let second = observed_checked_header(&calls, 2, "x-second", "contains\nnewline", b"second-failed");

    let response: Response<EitherBody<Body, EitherBody<StreamBody, StreamBody>>> = (first, second, Body::from("discarded")).into_response();

    assert_eq!(calls.get(), 2);
    assert!(response.headers().get("x-first").is_none());
    assert_eq!(
        observe(response.into_body()).await.chunks,
        [Bytes::from_static(b"second-failed"), Bytes::from_static(b":second")]
    );
}

#[tokio::test]
async fn two_capable_failures_prefer_the_rightmost_part_deterministically() {
    let calls = Cell::new(0);
    let first = observed_checked_header(&calls, 1, "x-first", "contains\nnewline", b"first-failed");
    let second = observed_checked_header(&calls, 2, "x-second", "contains\nnewline", b"second-failed");

    let response: Response<EitherBody<Body, EitherBody<StreamBody, StreamBody>>> = (first, second, Body::from("discarded")).into_response();

    assert_eq!(calls.get(), 2, "the leftmost part must not run after the rightmost failure");
    assert_eq!(
        observe(response.into_body()).await.chunks,
        [Bytes::from_static(b"second-failed"), Bytes::from_static(b":second")]
    );
}

#[test]
fn part_failure_status_is_not_overwritten_by_a_surrounding_success_status() {
    let failure = || {
        CheckedHeader::new(
            HeaderName::from_static("x-checked"),
            "contains\nnewline",
            StreamingPartsRejection::streaming(b"part-failed"),
        )
    };

    let right_failure = (StatusCode::CREATED, failure(), Body::from("discarded")).into_response();
    assert_eq!(right_failure.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let left_failure = (failure(), StatusCode::CREATED, Body::from("discarded")).into_response();
    assert_eq!(left_failure.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_part_rejection_retains_its_concrete_stream_error_without_boxing() {
    let part = CheckedHeader::new(
        HeaderName::from_static("x-checked"),
        "contains\nnewline",
        StreamingPartsRejection::failing("part stream failed"),
    );
    let response: Response<EitherBody<Body, StreamBody>> = (part, Body::from("discarded")).into_response();
    let error = first_body_error(response.into_body()).await;

    assert_eq!(error, EitherBodyError::Right(StreamFailure("part stream failed")));
}

#[tokio::test]
async fn generated_router_absorbs_fallible_part_and_success_body_errors() {
    let router = response_router();
    let service = ResponseBodies;

    let success = router.route(&service, request("/fallible-parts/0"), &()).await;
    assert_eq!(success.headers()["x-checked"], "ready");
    assert_eq!(
        observe(success.into_body()).await.chunks,
        [Bytes::from_static(b"handler"), Bytes::from_static(b":second")]
    );

    let rejected = router.route(&service, request("/fallible-parts/1"), &()).await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let rejected = observe(rejected.into_body()).await;
    assert_eq!(
        rejected.chunks,
        [Bytes::from_static(b"part-rejected"), Bytes::from_static(b":second")]
    );
    assert_eq!(
        rejected.trailers.expect("generated part rejection retains trailers")["x-stream-complete"],
        "yes"
    );

    let rejected_error = router.route(&service, request("/fallible-parts/2"), &()).await;
    let rejected_error = first_body_error(rejected_error.into_body()).await;
    assert!(rejected_error.to_string().contains("handler response"));

    let success_error = router.route(&service, request("/fallible-parts/3"), &()).await;
    let success_error = first_body_error(success_error.into_body()).await;
    assert!(success_error.to_string().contains("handler response"));
}

#[tokio::test]
async fn configured_dynamic_and_explicit_boxed_routes_can_stream() {
    let router = response_router();
    let service = ResponseBodies;

    let dynamic = router.route(&service, request("/dynamic/plugin"), &()).await;
    assert_eq!(
        observe(dynamic.into_body()).await.chunks,
        [Bytes::from_static(b"plugin"), Bytes::from_static(b":dynamic")]
    );

    let boxed = router.route(&service, request("/boxed"), &()).await;
    let boxed = observe(boxed.into_body()).await;
    assert_eq!(boxed.chunks, [Bytes::from_static(b"boxed"), Bytes::from_static(b":second")]);
    assert_eq!(boxed.trailers.expect("boxed bodies preserve trailers")["x-stream-complete"], "yes");
}

#[tokio::test]
async fn generated_body_errors_propagate_without_boxing() {
    let router = response_router();
    let service = ResponseBodies;
    let response = router.route(&service, request("/failure"), &()).await;
    let mut body = pin!(response.into_body());

    let error = core::future::poll_fn(|context| body.as_mut().poll_frame(context))
        .await
        .expect("the failing body yields one frame")
        .expect_err("the frame carries the body error");

    assert!(error.to_string().contains("handler response"));
    assert!(body.as_ref().is_end_stream());
}

#[tokio::test]
async fn generated_body_errors_expose_their_concrete_source_after_erasure() {
    let router = response_router();
    let service = ResponseBodies;
    let response = router.route(&service, request("/failure"), &()).await;
    let error = first_body_error(response.into_body()).await;

    // The sum's own `Display` names the failing response, and the concrete
    // handler body error stays reachable through the source chain even though
    // the generated sum is private and unnameable.
    assert!(error.to_string().contains("handler response"));
    let source = core::error::Error::source(&error).expect("the sum forwards its active variant as the source");
    assert_eq!(
        source.downcast_ref::<StreamFailure>().expect("the concrete body error survives"),
        &StreamFailure("stream failed")
    );
}

#[tokio::test]
async fn explicit_box_body_preserves_its_concrete_error() {
    let mut body = pin!(BoxBody::new(StreamBody::failed("boxed failure")));
    let error = core::future::poll_fn(|context| body.as_mut().poll_frame(context))
        .await
        .expect("the boxed body yields one frame")
        .expect_err("the boxed frame carries the body error");

    assert_eq!(
        error
            .as_error()
            .downcast_ref::<StreamFailure>()
            .expect("the concrete error remains available"),
        &StreamFailure("boxed failure")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn core_routing_accepts_a_non_send_handler_future_and_response_body() {
    let router = response_router();
    let service = ResponseBodies;
    let response = router.route(&service, request("/local"), &()).await;

    assert_eq!(observe(response.into_body()).await.chunks[0], "local");
}

#[tokio::test]
async fn opaque_return_contract_is_usable_by_a_transport_adapter() {
    let router = response_router();
    let response = router.route(&ResponseBodies, request("/fixed"), &()).await;

    assert_eq!(transport_adapter(response).await, Bytes::from_static(b"fixed"));
}

#[tokio::test]
async fn transport_specific_send_bounds_are_inferred_when_every_variant_supports_them() {
    assert_eq!(core::mem::size_of_val(&PublicStaticBodiesResponseBody), 0);
    let response = PublicStaticBodies.route(request("/stream"), &()).await;

    assert_eq!(send_transport_adapter(response).await, Bytes::from_static(b"transport:second"));
}

fn response_router() -> ResponseBodiesRouter {
    ResponseBodies::router_builder()
        .add_dynamic_stream("GET", "/dynamic/{name}")
        .build()
        .expect("the dynamic streaming route is valid")
}

fn request(path: &str) -> Request<()> {
    Request::builder().uri(path).body(()).expect("the test request metadata is valid")
}

fn observed_checked_header<'a>(
    calls: &'a Cell<u8>,
    id: u8,
    name: &'static str,
    value: &'static str,
    rejection_label: &'static [u8],
) -> ObservedCheckedHeader<'a> {
    ObservedCheckedHeader {
        part: CheckedHeader::new(
            HeaderName::from_static(name),
            value,
            StreamingPartsRejection::streaming(rejection_label),
        ),
        calls,
        id,
    }
}

struct Observation {
    chunks: Vec<Bytes>,
    trailers: Option<HeaderMap>,
    ended: bool,
}

async fn first_body_error<B>(body: B) -> B::Error
where
    B: http_body::Body<Data = Bytes>,
    B::Error: fmt::Debug,
{
    let mut body = pin!(body);
    core::future::poll_fn(|context| body.as_mut().poll_frame(context))
        .await
        .expect("the failing response body yields one frame")
        .expect_err("the first frame contains the expected body error")
}

async fn observe<B>(body: B) -> Observation
where
    B: http_body::Body<Data = Bytes>,
    B::Error: fmt::Debug,
{
    let mut body = pin!(body);
    let mut chunks = Vec::new();
    let mut trailers = None;
    while let Some(frame) = core::future::poll_fn(|context| body.as_mut().poll_frame(context)).await {
        let frame = frame.expect("the observed response body succeeds");
        match frame.into_data() {
            Ok(data) => chunks.push(data),
            Err(frame) => {
                trailers = Some(frame.into_trailers().expect("a non-data response frame contains trailers"));
            }
        }
    }
    Observation {
        chunks,
        trailers,
        ended: body.as_ref().is_end_stream(),
    }
}

async fn transport_adapter<B>(response: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: std::error::Error,
{
    let observation = observe(response.into_body()).await;
    let mut combined = BytesMut::new();
    for chunk in observation.chunks {
        combined.extend_from_slice(&chunk);
    }
    combined.freeze()
}

async fn send_transport_adapter<B>(response: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: std::error::Error + Send + Sync + 'static,
{
    transport_adapter(response).await
}
