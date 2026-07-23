// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for typed route policy.

#![deny(private_bounds, private_interfaces)]

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::fmt;
#[cfg(not(miri))]
use std::future::Future as _;
use std::pin::Pin;
#[cfg(not(miri))]
use std::pin::pin;
use std::rc::Rc;
#[cfg(not(miri))]
use std::task::Waker;
use std::task::{Context, Poll};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http::HeaderMap;
use http::header::{ACCEPT, CONTENT_TYPE, HOST, HeaderName, HeaderValue};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::{Body, IntoResponse, Response};
use routerama::route::{
    BodyRejection, BytesBody, FromRequestBody, FromRequestParts, Request, RequestParts, RouteFailure, StatusCode, router,
};

#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[derive(Clone, Copy, Debug)]
struct PartsFailure;

impl IntoResponse for PartsFailure {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        StatusCode::BAD_REQUEST.into_response()
    }
}

struct RejectParts;

impl<S: ?Sized> FromRequestParts<'_, S> for RejectParts {
    type Rejection = PartsFailure;

    fn from_request_parts(_parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        Err(PartsFailure)
    }
}

#[derive(Clone, Copy, Debug)]
struct OtherFailure;

impl IntoResponse for OtherFailure {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        StatusCode::CONFLICT.into_response()
    }
}

struct RejectOther;

impl<S: ?Sized> FromRequestParts<'_, S> for RejectOther {
    type Rejection = OtherFailure;

    fn from_request_parts(_parts: &RequestParts, _state: &S) -> Result<Self, Self::Rejection> {
        Err(OtherFailure)
    }
}

struct PolicyApi {
    calls: Cell<u32>,
    extracts: Cell<u32>,
}

struct Probe<const ID: u32>;

impl<const ID: u32> FromRequestParts<'_, PolicyApi> for Probe<ID> {
    type Rejection = Infallible;

    fn from_request_parts(_parts: &RequestParts, state: &PolicyApi) -> Result<Self, Self::Rejection> {
        state.extracts.set(state.extracts.get() * 10 + ID);
        Ok(Self)
    }
}

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "Cells record policy ordering and router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl PolicyApi {
    #[route(GET, "/items/{id}", host = "priority.example", priority = 30)]
    async fn by_host(&self, id: u32, probe: Probe<1>) -> String {
        let _ = probe;
        self.calls.set(self.calls.get() * 10 + 1);
        format!("host:{id}")
    }

    #[route(GET, "/items/{id}", consumes = "application/json", priority = 20)]
    async fn by_content_type(&self, id: u32, probe: Probe<2>) -> String {
        let _ = probe;
        self.calls.set(self.calls.get() * 10 + 2);
        format!("consumes:{id}")
    }

    #[route(GET, "/items/{id}", produces = "text/plain", priority = 10)]
    async fn by_accept(&self, id: u32, probe: Probe<3>) -> String {
        let _ = probe;
        self.calls.set(self.calls.get() * 10 + 3);
        format!("produces:{id}")
    }

    #[route(GET, "/caught")]
    async fn caught(&self, rejected: RejectParts) -> StatusCode {
        let _ = rejected;
        self.calls.set(self.calls.get() * 10 + 9);
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/uncaught")]
    async fn uncaught(&self, rejected: RejectOther) -> StatusCode {
        let _ = rejected;
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/decode/{value}")]
    async fn decode(&self, value: Cow<'_, str>) -> String {
        value.into_owned()
    }

    #[route(GET, "/fallback-host", host = "one.example", priority = 2)]
    async fn fallback_host_one(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/fallback-host", host = "two.example", priority = 1)]
    async fn fallback_host_two(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/fallback-consumes", host = "one.example", priority = 2)]
    async fn fallback_consumes_host(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/fallback-consumes", consumes = "application/json", priority = 1)]
    async fn fallback_consumes_json(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[catch(PartsFailure, from = RejectParts)]
    async fn catch_parts(&self, _failure: PartsFailure) -> (StatusCode, &'static str) {
        self.calls.set(self.calls.get() * 10 + 4);
        (StatusCode::IM_A_TEAPOT, "caught")
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> (StatusCode, String) {
        self.calls.set(self.calls.get() * 10 + 5);
        (failure.status(), failure.to_string())
    }
}

#[tokio::test]
async fn overlap_candidates_follow_priority_and_do_not_extract_before_selection() {
    let api = PolicyApi {
        calls: Cell::new(0),
        extracts: Cell::new(0),
    };
    let request = Request::get("/items/7")
        .header(HOST, "priority.example")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/plain")
        .body(())
        .expect("test metadata is valid");
    let response = api.route(request, &api).await;
    assert_eq!(body(response).await, b"host:7"[..]);
    assert_eq!(api.calls.get(), 1);
    assert_eq!(api.extracts.get(), 1);

    let request = Request::get("/items/8")
        .header(HOST, "other.example")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/plain")
        .body(())
        .expect("test metadata is valid");
    let response = api.route(request, &api).await;
    assert_eq!(body(response).await, b"consumes:8"[..]);
    assert_eq!(api.calls.get(), 12);
    assert_eq!(api.extracts.get(), 12);

    let request = Request::get("/items/9")
        .header(HOST, "other.example")
        .header(CONTENT_TYPE, "text/plain")
        .header(ACCEPT, "text/plain")
        .body(())
        .expect("test metadata is valid");
    let response = api.route(request, &api).await;
    assert_eq!(body(response).await, b"produces:9"[..]);
    assert_eq!(api.calls.get(), 123);
    assert_eq!(api.extracts.get(), 123);
}

struct ProbeBody<const ID: u32>(Vec<u8>);

impl<const ID: u32> FromRequestBody<Cell<u32>, Vec<u8>> for ProbeBody<ID> {
    type Rejection = Infallible;

    fn from_request_body(
        _parts: &RequestParts,
        body: Vec<u8>,
        state: &Cell<u32>,
    ) -> impl core::future::Future<Output = Result<Self, Self::Rejection>> {
        state.set(state.get() * 10 + ID);
        core::future::ready(Ok(Self(body)))
    }
}

struct BodySelection;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl BodySelection {
    #[route(POST, "/body-choice", host = "one.example", priority = 2)]
    async fn one(&self, #[body] body: ProbeBody<1>) -> Vec<u8> {
        body.0
    }

    #[route(POST, "/body-choice", host = "two.example", priority = 1)]
    async fn two(&self, #[body] body: ProbeBody<2>) -> Vec<u8> {
        body.0
    }
}

#[tokio::test]
async fn overlap_selection_moves_the_body_only_to_the_selected_extractor() {
    let effects = Cell::new(0);
    let request = Request::post("/body-choice")
        .header(HOST, "two.example")
        .body(b"selected".to_vec())
        .expect("valid request");
    let response = BodySelection.route(request, &effects).await;
    assert_eq!(body(response).await, b"selected"[..]);
    assert_eq!(effects.get(), 2);
}

#[tokio::test]
async fn overlap_failure_uses_the_deepest_stage_and_routes_through_fallback() {
    let api = PolicyApi {
        calls: Cell::new(0),
        extracts: Cell::new(0),
    };
    let request = Request::get("/items/7")
        .header(HOST, "other.example")
        .header(CONTENT_TYPE, "text/plain")
        .header(ACCEPT, "application/json")
        .body(())
        .expect("test metadata is valid");
    let response = api.route(request, &api).await;
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        body(response).await,
        b"no route candidate for `/items/7` could produce an acceptable representation"[..]
    );
    assert_eq!(api.calls.get(), 5);
    assert_eq!(api.extracts.get(), 0);
}

struct DefaultOverlap;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl DefaultOverlap {
    #[route(GET, "/host", host = "one.example", priority = 2)]
    async fn host_one(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/host", host = "two.example", priority = 1)]
    async fn host_two(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/consumes", host = "one.example", priority = 2)]
    async fn consumes_host(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/consumes", consumes = "application/json", priority = 1)]
    async fn consumes_json(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/produces", consumes = "application/json", priority = 2)]
    async fn produces_consumes(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/produces", produces = "application/json", priority = 1)]
    async fn produces_json(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/default", host = "one.example", priority = 2)]
    async fn preferred_default(&self) -> &'static str {
        "preferred"
    }

    #[route(GET, "/default", priority = 1)]
    async fn unconditional_default(&self) -> &'static str {
        "default"
    }
}

#[tokio::test]
async fn uncaught_overlap_failure_status_uses_the_deepest_stage_across_candidates() {
    let host = Request::get("/host").header(HOST, "other.example").body(()).expect("valid request");
    assert_eq!(DefaultOverlap.route(host, &()).await.status(), StatusCode::NOT_FOUND);

    let consumes = Request::get("/consumes")
        .header(HOST, "other.example")
        .header(CONTENT_TYPE, "text/plain")
        .body(())
        .expect("valid request");
    assert_eq!(
        DefaultOverlap.route(consumes, &()).await.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let produces = Request::get("/produces")
        .header(CONTENT_TYPE, "text/plain")
        .header(ACCEPT, "text/plain")
        .body(())
        .expect("valid request");
    assert_eq!(DefaultOverlap.route(produces, &()).await.status(), StatusCode::NOT_ACCEPTABLE);
}

#[tokio::test]
async fn predicate_free_overlap_default_needs_no_rejection_body_variant() {
    let preferred = Request::get("/default")
        .header(HOST, "one.example")
        .body(())
        .expect("valid request");
    assert_eq!(body(DefaultOverlap.route(preferred, &()).await).await, b"preferred"[..]);

    let default = Request::get("/default")
        .header(HOST, "other.example")
        .body(())
        .expect("valid request");
    assert_eq!(body(DefaultOverlap.route(default, &()).await).await, b"default"[..]);
}

struct AliasPolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl AliasPolicy {
    #[route(GET, "/shared", host = "alias.example", priority = 2)]
    #[route(GET, "/solo", host = "alias.example")]
    async fn preferred(&self) -> &'static str {
        "preferred"
    }

    #[route(GET, "/shared", priority = 1)]
    async fn alternate(&self) -> &'static str {
        "alternate"
    }
}

#[tokio::test]
async fn aliases_participate_in_overlap_only_on_the_declaration_that_collides() {
    let shared = Request::get("/shared")
        .header(HOST, "alias.example")
        .header(CONTENT_TYPE, "application/json")
        .body(())
        .expect("valid request");
    assert_eq!(body(AliasPolicy.route(shared, &()).await).await, b"preferred"[..]);

    let default = Request::get("/shared")
        .header(HOST, "other.example")
        .body(())
        .expect("valid request");
    assert_eq!(body(AliasPolicy.route(default, &()).await).await, b"alternate"[..]);

    let solo = Request::get("/solo").header(HOST, "alias.example").body(()).expect("valid request");
    assert_eq!(body(AliasPolicy.route(solo, &()).await).await, b"preferred"[..]);
}

struct OwnedCapturePolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl OwnedCapturePolicy {
    #[route(GET, "/owned/{name}", host = "one.example", priority = 2)]
    async fn one(&self, name: String) -> String {
        format!("one:{name}")
    }

    #[route(GET, "/owned/{name}", host = "two.example", priority = 1)]
    async fn two(&self, name: String) -> String {
        format!("two:{name}")
    }
}

#[tokio::test]
async fn one_owned_capture_conversion_is_moved_only_into_the_selected_candidate() {
    let request = Request::get("/owned/value%20with%20spaces")
        .header(HOST, "two.example")
        .body(())
        .expect("valid request");
    assert_eq!(
        body(OwnedCapturePolicy.route(request, &()).await).await,
        b"two:value with spaces"[..]
    );
}

#[tokio::test]
async fn fallback_handles_not_found_and_capture_conversion_without_erasing_diagnostics() {
    let api = PolicyApi {
        calls: Cell::new(0),
        extracts: Cell::new(0),
    };

    let response = api.route(Request::get("/missing").body(()).expect("valid request"), &api).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(response).await, b"no route matched path `/missing`"[..]);

    let response = api.route(Request::get("/items/nope").body(()).expect("valid request"), &api).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await, b"failed to parse capture for field `id`"[..]);

    let response = api.route(Request::get("/decode/%FF").body(()).expect("valid request"), &api).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await, b"failed to percent-decode capture for field `value`"[..]);

    let host = Request::get("/fallback-host")
        .header(HOST, "other.example")
        .body(())
        .expect("valid request");
    let response = api.route(host, &api).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body(response).await,
        b"no route candidate for `/fallback-host` accepted the request host"[..]
    );

    let consumes = Request::get("/fallback-consumes")
        .header(HOST, "other.example")
        .header(CONTENT_TYPE, "text/plain")
        .body(())
        .expect("valid request");
    let response = api.route(consumes, &api).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body(response).await,
        b"no route candidate for `/fallback-consumes` accepted the request content type"[..]
    );

    for failure in [
        RouteFailure::MalformedPath { path: "/bad?query" },
        RouteFailure::MissingCapture { field: "id" },
    ] {
        let expected = failure.to_string();
        let response = api.fallback(failure).await.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(body(response).await, expected.as_bytes());
    }
}

#[tokio::test]
async fn a_typed_catcher_replaces_only_its_exact_extractor_rejection() {
    let api = PolicyApi {
        calls: Cell::new(0),
        extracts: Cell::new(0),
    };
    let response = api.route(Request::get("/caught").body(()).expect("valid request"), &api).await;

    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(body(response).await, b"caught"[..]);
    assert_eq!(api.calls.get(), 4);

    let uncaught = api.route(Request::get("/uncaught").body(()).expect("valid request"), &api).await;
    assert_eq!(uncaught.status(), StatusCode::CONFLICT);
    assert_eq!(api.calls.get(), 4, "the routing fallback must not catch extractor failures");
}

struct BodyCatcher;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl BodyCatcher {
    #[route(POST, "/body")]
    async fn body(&self, #[body] body: BytesBody<2>) -> StatusCode {
        let _ = body;
        StatusCode::NO_CONTENT
    }

    #[catch(BodyRejection<Infallible>)]
    async fn catch_body(&self, rejection: BodyRejection<Infallible>) -> (StatusCode, &'static str) {
        match rejection {
            BodyRejection::TooLarge(_) => (StatusCode::UNPROCESSABLE_ENTITY, "body-caught"),
            BodyRejection::Transport(_) => {
                unreachable!("routerama::response::Body uses an Infallible transport error")
            }
            BodyRejection::InvalidUtf8(_) => (StatusCode::BAD_REQUEST, "utf8"),
        }
    }
}

#[tokio::test]
async fn built_in_body_rejections_can_be_caught_exactly() {
    let request = Request::post("/body").body(Body::from("long")).expect("test metadata is valid");
    let response = BodyCatcher.route(request, &()).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body(response).await, b"body-caught"[..]);
}

#[derive(Clone, Copy, Debug)]
struct StreamFailure(&'static str);

impl fmt::Display for StreamFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for StreamFailure {}

struct PolicyStream {
    frames: VecDeque<Result<Frame<Bytes>, StreamFailure>>,
}

impl PolicyStream {
    fn catcher() -> Self {
        let mut trailers = HeaderMap::new();
        trailers.insert(HeaderName::from_static("x-policy-trailer"), HeaderValue::from_static("caught"));
        Self {
            frames: [
                Ok(Frame::data(Bytes::from_static(b"caught-frame"))),
                Ok(Frame::trailers(trailers)),
                Err(StreamFailure("catcher-stream-error")),
            ]
            .into(),
        }
    }

    fn fallback() -> Self {
        Self {
            frames: [
                Ok(Frame::data(Bytes::from_static(b"fallback-frame"))),
                Err(StreamFailure("fallback-stream-error")),
            ]
            .into(),
        }
    }
}

impl HttpBody for PolicyStream {
    type Data = Bytes;
    type Error = StreamFailure;

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

struct StreamingPolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl StreamingPolicy {
    #[route(GET, "/caught")]
    async fn caught(&self, reject: RejectParts) -> StatusCode {
        let _ = reject;
        StatusCode::NO_CONTENT
    }

    #[catch(PartsFailure, from = RejectParts)]
    async fn catch_parts(&self, _failure: PartsFailure) -> Response<PolicyStream> {
        Response::new(PolicyStream::catcher())
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> Response<PolicyStream> {
        let mut response = Response::new(PolicyStream::fallback());
        *response.status_mut() = failure.status();
        response
    }
}

#[tokio::test]
async fn catcher_and_fallback_streams_preserve_frames_trailers_and_errors() {
    let caught = StreamingPolicy
        .route(Request::get("/caught").body(()).expect("valid request"), &())
        .await;
    let mut caught = caught.into_body();
    let first = caught
        .frame()
        .await
        .expect("data frame")
        .expect("stream succeeds")
        .into_data()
        .expect("data");
    assert_eq!(first, b"caught-frame"[..]);
    let trailers = caught
        .frame()
        .await
        .expect("trailer frame")
        .expect("stream succeeds")
        .into_trailers()
        .expect("trailers");
    assert_eq!(trailers["x-policy-trailer"], "caught");
    let error = caught.frame().await.expect("error frame").expect_err("stream error is retained");
    assert!(error.to_string().contains("extractor catcher response"), "{error}");

    let fallback = StreamingPolicy
        .route(Request::get("/missing").body(()).expect("valid request"), &())
        .await;
    assert_eq!(fallback.status(), StatusCode::NOT_FOUND);
    let mut fallback = fallback.into_body();
    let data = fallback
        .frame()
        .await
        .expect("data frame")
        .expect("first frame succeeds")
        .into_data()
        .expect("data");
    assert_eq!(data, b"fallback-frame"[..]);
    let error = fallback.frame().await.expect("error frame").expect_err("stream error is retained");
    assert!(error.to_string().contains("routing fallback response"), "{error}");
}

struct LocalPolicyBody {
    frame: Option<Bytes>,
    _local: Rc<()>,
}

#[derive(Debug)]
struct LocalPolicyError {
    _local: Rc<()>,
}

impl fmt::Display for LocalPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("local policy error")
    }
}

impl HttpBody for LocalPolicyBody {
    type Data = Bytes;
    type Error = LocalPolicyError;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.frame.take().map(|frame| Ok(Frame::data(frame))))
    }
}

struct LocalPolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "this policy intentionally proves local futures and bodies while policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl LocalPolicy {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/caught")]
    async fn caught(&self, reject: RejectParts) -> StatusCode {
        let _ = reject;
        StatusCode::NO_CONTENT
    }

    #[catch(PartsFailure, from = RejectParts)]
    async fn catch_parts(&self, _failure: PartsFailure) -> Response<LocalPolicyBody> {
        let local = Rc::new(());
        core::future::ready(()).await;
        Response::new(LocalPolicyBody {
            frame: Some(Bytes::from_static(b"local-catcher")),
            _local: local,
        })
    }

    #[fallback]
    async fn fallback(&self, _failure: RouteFailure<'_>) -> Response<LocalPolicyBody> {
        let local = Rc::new(());
        core::future::ready(()).await;
        Response::new(LocalPolicyBody {
            frame: Some(Bytes::from_static(b"local")),
            _local: local,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn policy_futures_and_bodies_need_not_be_send() {
    let response = LocalPolicy
        .route(Request::get("/missing").body(()).expect("valid request"), &())
        .await;
    assert_eq!(body(response).await, b"local"[..]);

    let caught = LocalPolicy
        .route(Request::get("/caught").body(()).expect("valid request"), &())
        .await;
    assert_eq!(body(caught).await, b"local-catcher"[..]);
}

struct MixedCollision;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl MixedCollision {
    #[route(GET, "/same")]
    async fn fixed(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(dynamic)]
    async fn configured(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[test]
fn configured_dynamic_routes_cannot_overlap_generated_static_routes() {
    let error = MixedCollision::router_builder()
        .add_configured("GET", "/same")
        .build()
        .expect_err("runtime overlap is rejected instead of adding candidate indirection");
    assert!(error.to_string().contains("conflicting routes"), "{error}");
}

struct DynamicPolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl DynamicPolicy {
    #[route(GET, "/fixed")]
    async fn fixed(&self) -> &'static str {
        "fixed"
    }

    #[route(dynamic)]
    async fn dynamic(&self, reject: RejectParts) -> StatusCode {
        let _ = reject;
        StatusCode::NO_CONTENT
    }

    #[catch(PartsFailure, from = RejectParts)]
    async fn catch_parts(&self, _failure: PartsFailure) -> (StatusCode, &'static str) {
        (StatusCode::IM_A_TEAPOT, "dynamic-caught")
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> (StatusCode, &'static str) {
        (failure.status(), "dynamic-fallback")
    }
}

#[tokio::test]
async fn mixed_configured_services_pass_the_service_to_catchers_and_fallbacks() {
    let router = DynamicPolicy::router_builder()
        .add_dynamic("GET", "/dynamic")
        .build()
        .expect("dynamic registration is valid");
    let caught = router
        .route(&DynamicPolicy, Request::get("/dynamic").body(()).expect("valid request"), &())
        .await;
    assert_eq!(caught.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(body(caught).await, b"dynamic-caught"[..]);

    let missing = router
        .route(&DynamicPolicy, Request::get("/missing").body(()).expect("valid request"), &())
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(missing).await, b"dynamic-fallback"[..]);
}

struct AllocationPolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl AllocationPolicy {
    #[route(GET, "/plain")]
    async fn plain(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/overlap", host = "allocation.example", priority = 2)]
    async fn overlap_host(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/overlap", consumes = "application/json", priority = 1)]
    async fn overlap_consumes(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/caught")]
    async fn caught(&self, reject: RejectParts) -> StatusCode {
        let _ = reject;
        StatusCode::NO_CONTENT
    }

    #[catch(PartsFailure, from = RejectParts)]
    async fn catch_parts(&self, _failure: PartsFailure) -> StatusCode {
        StatusCode::IM_A_TEAPOT
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
        failure.status()
    }
}

#[test]
#[cfg(not(miri))]
fn prepared_plain_overlap_fallback_and_catcher_paths_allocate_zero() {
    let session = Session::new().no_stdout().no_file();
    let mut context = Context::from_waker(Waker::noop());
    let api = AllocationPolicy;
    let requests = [
        (
            "policy_plain",
            Request::get("/plain").body(()).expect("valid request"),
            StatusCode::NO_CONTENT,
        ),
        (
            "policy_overlap",
            Request::get("/overlap")
                .header(HOST, "other.example")
                .header(CONTENT_TYPE, "application/json")
                .body(())
                .expect("valid request"),
            StatusCode::NO_CONTENT,
        ),
        (
            "policy_fallback",
            Request::get("/missing").body(()).expect("valid request"),
            StatusCode::NOT_FOUND,
        ),
        (
            "policy_catcher",
            Request::get("/caught").body(()).expect("valid request"),
            StatusCode::IM_A_TEAPOT,
        ),
    ];

    for (name, request, expected) in requests {
        let mut future = pin!(api.route(request, &()));
        let operation = session.operation(name);
        let response = {
            let _span = operation.measure_thread();
            match future.as_mut().poll(&mut context) {
                Poll::Ready(response) => std::hint::black_box(response),
                Poll::Pending => panic!("prepared policy dispatch has no pending operation"),
            }
        };
        assert_eq!(response.status(), expected);
        assert_eq!(operation.total_bytes_allocated(), 0, "{name}");
    }
}

#[test]
fn route_failure_defaults_cover_every_typed_class() {
    for (failure, status) in [
        (RouteFailure::NotFound { path: "/x" }, StatusCode::NOT_FOUND),
        (RouteFailure::MalformedPath { path: "/x?y" }, StatusCode::BAD_REQUEST),
        (RouteFailure::MissingCapture { field: "id" }, StatusCode::BAD_REQUEST),
        (RouteFailure::InvalidCapture { field: "id" }, StatusCode::BAD_REQUEST),
        (RouteFailure::UndecodableCapture { field: "id" }, StatusCode::BAD_REQUEST),
        (RouteFailure::HostMismatch { path: "/x" }, StatusCode::NOT_FOUND),
        (
            RouteFailure::UnsupportedMediaType { path: "/x" },
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (RouteFailure::NotAcceptable { path: "/x" }, StatusCode::NOT_ACCEPTABLE),
    ] {
        assert_eq!(failure.status(), status);
        assert_eq!(failure.into_response().status(), status);
    }
}

async fn body<B>(body: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: std::fmt::Debug,
{
    body.into_body().collect().await.expect("body succeeds").to_bytes()
}
