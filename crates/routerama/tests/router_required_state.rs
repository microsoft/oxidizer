// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral, layout, and allocation coverage for fixed router state.

#![deny(private_bounds, private_interfaces)]

use std::cell::Cell;
#[cfg(not(miri))]
use std::future::Future as _;
#[cfg(not(miri))]
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
#[cfg(not(miri))]
use std::task::{Context, Poll, Waker};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::{
    BodyStateWitness, FromRef, FromRequestBody, FromRequestParts, Request, RequestParts, RouteFailure, State, StatusCode, router,
};

#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn total_bytes_allocated(session: &Session, operation_name: &str) -> u64 {
    let missing_operation = format!("operation {operation_name:?} must match a name registered with Session::operation on this session");

    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == operation_name).then(|| operation.total_bytes_allocated()))
        .expect(&missing_operation)
}

#[derive(Clone)]
struct AppState {
    label: Arc<str>,
    sequence: u32,
    body_calls: Rc<Cell<u32>>,
}

#[derive(Clone)]
struct Label(Arc<str>);

impl FromRef<AppState> for Label {
    fn from_ref(input: &AppState) -> Self {
        Self(Arc::clone(&input.label))
    }
}

#[derive(Clone, Copy)]
struct Sequence(u32);

impl FromRef<AppState> for Sequence {
    fn from_ref(input: &AppState) -> Self {
        Self(input.sequence)
    }
}

struct MatchingHeader<'request>(&'request str);

impl<'request> FromRequestParts<'request, AppState> for MatchingHeader<'request> {
    type Rejection = StatusCode;

    fn from_request_parts(parts: &'request RequestParts, state: &AppState) -> Result<Self, Self::Rejection> {
        let value = parts
            .headers
            .get("x-state")
            .and_then(|value| value.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if value == state.label.as_ref() {
            Ok(Self(value))
        } else {
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

struct StateBody(Vec<u8>);

impl FromRequestBody<AppState, Vec<u8>> for StateBody {
    type Rejection = StatusCode;

    fn from_request_body(parts: &RequestParts, mut body: Vec<u8>, state: &AppState) -> impl Future<Output = Result<Self, Self::Rejection>> {
        let _ = parts;
        state.body_calls.set(state.body_calls.get() + 1);
        let result = if body == b"reject" {
            Err(StatusCode::IM_A_TEAPOT)
        } else {
            body.extend_from_slice(state.label.as_bytes());
            Ok(Self(body))
        };
        core::future::ready(result)
    }
}

impl BodyStateWitness<AppState, StatusCode> for StateBody {
    type RequestBody = Vec<u8>;
}

struct FixedApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "the fixed state is intentionally local and router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl FixedApi {
    #[route(GET, "/same")]
    async fn same(&self, state: State<AppState>) -> String {
        format!("{}:{}", state.label, state.sequence)
    }

    #[route(GET, "/projected")]
    async fn projected(&self, label: State<Label>, sequence: State<Sequence>) -> String {
        format!("{}:{}", label.0.0, sequence.0.0)
    }

    #[route(GET, "/borrowed")]
    async fn borrowed(&self, header: MatchingHeader<'_>, label: State<Label>) -> String {
        assert_eq!(header.0, label.0.0.as_ref());
        header.0.to_owned()
    }

    #[route(POST, "/body")]
    async fn body(&self, #[body] body: StateBody, sequence: State<Sequence>) -> String {
        format!("{}:{}", String::from_utf8(body.0).expect("test body is UTF-8"), sequence.0.0)
    }

    #[route(GET, "/no-state")]
    async fn no_state(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[catch(StatusCode, from = StateBody)]
    async fn catch_body(&self, rejection: StatusCode) -> StatusCode {
        rejection
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fixed_state_supports_clones_projections_borrowed_and_body_extractors() {
    let state = app_state();

    let same = FixedApi.route(vector_request("GET", "/same"), &state).await;
    assert_eq!(body(same).await, b"required:41"[..]);

    let projected = FixedApi.route(vector_request("GET", "/projected"), &state).await;
    assert_eq!(body(projected).await, b"required:41"[..]);

    let borrowed = Request::get("/borrowed")
        .header("x-state", "required")
        .body(Vec::new())
        .expect("test request metadata is valid");
    assert_eq!(body(FixedApi.route(borrowed, &state).await).await, b"required"[..]);

    let extracted = FixedApi
        .route(
            Request::post("/body")
                .body(b"body:".to_vec())
                .expect("test request metadata is valid"),
            &state,
        )
        .await;
    assert_eq!(body(extracted).await, b"body:required:41"[..]);
    assert_eq!(state.body_calls.get(), 1);

    let rejected = FixedApi
        .route(
            Request::post("/body")
                .body(b"reject".to_vec())
                .expect("test request metadata is valid"),
            &state,
        )
        .await;
    assert_eq!(rejected.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(state.body_calls.get(), 2);

    let no_state = FixedApi.route(vector_request("GET", "/no-state"), &state).await;
    assert_eq!(no_state.status(), StatusCode::NO_CONTENT);
}

struct DynamicOnly;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl DynamicOnly {
    #[route(dynamic)]
    async fn named(&self, #[capture] name: String, sequence: State<Sequence>) -> String {
        format!("{name}:{}", sequence.0.0)
    }
}

struct Mixed;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl Mixed {
    #[route(GET, "/fixed")]
    async fn fixed(&self, label: State<Label>) -> String {
        label.0.0.to_string()
    }

    #[route(dynamic)]
    async fn dynamic(&self, sequence: State<Sequence>) -> String {
        sequence.0.0.to_string()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn configured_dynamic_and_mixed_routers_share_the_fixed_state_signature() {
    let state = app_state();
    let dynamic = DynamicOnly::router_builder()
        .add_named("GET", "/dynamic/{name}")
        .build()
        .expect("dynamic route is valid");
    assert_eq!(
        body(
            dynamic
                .route(
                    &DynamicOnly,
                    Request::get("/dynamic/plugin").body(()).expect("valid request"),
                    &state,
                )
                .await
        )
        .await,
        b"plugin:41"[..]
    );

    let mixed = Mixed::router_builder()
        .add_dynamic("GET", "/dynamic")
        .build()
        .expect("mixed routes are valid");
    assert_eq!(
        body(
            mixed
                .route(&Mixed, Request::get("/fixed").body(()).expect("valid request"), &state,)
                .await
        )
        .await,
        b"required"[..]
    );
    assert_eq!(
        body(
            mixed
                .route(&Mixed, Request::get("/dynamic").body(()).expect("valid request"), &state,)
                .await
        )
        .await,
        b"41"[..]
    );
}

#[derive(Clone, Copy)]
struct PolicyRejection;

impl routerama::response::IntoResponse for PolicyRejection {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        routerama::response::IntoResponse::into_response(StatusCode::BAD_REQUEST)
    }
}

struct RejectFromState;

impl FromRequestParts<'_, AppState> for RejectFromState {
    type Rejection = PolicyRejection;

    fn from_request_parts(parts: &RequestParts, state: &AppState) -> Result<Self, Self::Rejection> {
        let _ = (parts, state);
        Err(PolicyRejection)
    }
}

struct FixedPolicy;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router policy methods must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl FixedPolicy {
    #[route(GET, "/overlap", host = "one.example", priority = 2)]
    async fn preferred(&self, sequence: State<Sequence>) -> String {
        format!("preferred:{}", sequence.0.0)
    }

    #[route(GET, "/overlap", priority = 1)]
    async fn default(&self) -> &'static str {
        "default"
    }

    #[route(GET, "/caught")]
    async fn caught(&self, rejected: RejectFromState) -> StatusCode {
        let _ = rejected;
        StatusCode::NO_CONTENT
    }

    #[catch(PolicyRejection, from = RejectFromState)]
    async fn catch(&self, _rejection: PolicyRejection) -> StatusCode {
        StatusCode::IM_A_TEAPOT
    }

    #[fallback]
    async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
        failure.status()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fixed_state_preserves_overlaps_catchers_and_fallbacks() {
    let state = app_state();
    let preferred = Request::get("/overlap")
        .header("host", "one.example")
        .body(())
        .expect("valid request");
    assert_eq!(body(FixedPolicy.route(preferred, &state).await).await, b"preferred:41"[..]);

    let fallback_candidate = Request::get("/overlap")
        .header("host", "other.example")
        .body(())
        .expect("valid request");
    assert_eq!(body(FixedPolicy.route(fallback_candidate, &state).await).await, b"default"[..]);

    let caught = FixedPolicy
        .route(Request::get("/caught").body(()).expect("valid request"), &state)
        .await;
    assert_eq!(caught.status(), StatusCode::IM_A_TEAPOT);

    let missing = FixedPolicy
        .route(Request::get("/missing").body(()).expect("valid request"), &state)
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

struct GenericApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl GenericApi {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[tokio::test]
async fn bare_router_remains_reusable_with_unrelated_state_types() {
    let unit = GenericApi.route(Request::get("/").body(()).expect("valid request"), &()).await;
    assert_eq!(unit.status(), StatusCode::NO_CONTENT);

    let text = String::from("unrelated");
    let string = GenericApi.route(Request::get("/").body(()).expect("valid request"), &text).await;
    assert_eq!(string.status(), StatusCode::NO_CONTENT);
}

struct OuterMarker(u32);

mod qualified_state_paths {
    use super::{FromRef, OuterMarker, Request, State, StatusCode, router};

    #[derive(Clone)]
    pub(super) struct Shared<T>(pub(super) T);

    #[derive(Clone, Copy)]
    struct Projected(u32);

    impl FromRef<self::Shared<super::OuterMarker>> for Projected {
        fn from_ref(input: &self::Shared<super::OuterMarker>) -> Self {
            Self(input.0.0)
        }
    }

    struct Api;

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
    )]
    #[router(state = self::Shared<super::OuterMarker>)]
    impl self::Api {
        #[route(GET, "/")]
        async fn home(&self, projected: State<Projected>) -> StatusCode {
            assert_eq!(projected.0.0, 73);
            StatusCode::NO_CONTENT
        }
    }

    pub(super) async fn route() -> StatusCode {
        Api.route(Request::get("/").body(()).expect("valid request"), &Shared(OuterMarker(73)))
            .await
            .status()
    }
}

#[tokio::test]
async fn fixed_state_preserves_self_super_and_generic_source_paths() {
    assert_eq!(qualified_state_paths::route().await, StatusCode::NO_CONTENT);
}

struct UnsizedState;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = str)]
impl UnsizedState {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[tokio::test]
async fn fixed_state_may_be_unsized_when_shared_by_reference() {
    let response = UnsizedState
        .route(Request::get("/").body(()).expect("valid request"), "shared")
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

struct FixedLayout;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl FixedLayout {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[test]
#[cfg(not(miri))]
fn fixed_state_only_specializes_generics_without_layout_or_allocation_cost() {
    let state = app_state();
    let fixed_request = Request::get("/").body(()).expect("valid request");
    let generic_request = Request::get("/").body(()).expect("valid request");
    let fixed_future = FixedLayout.route(fixed_request, &state);
    let generic_future = GenericApi.route(generic_request, &state);
    assert_eq!(size_of_val(&fixed_future), size_of_val(&generic_future));
    drop((fixed_future, generic_future));

    let session = Session::new().no_stdout().no_file();
    let mut context = Context::from_waker(Waker::noop());
    for (name, fixed) in [("fixed_state", true), ("generic_state", false)] {
        let operation = session.operation(name);
        let status = if fixed {
            let request = Request::get("/").body(()).expect("valid request");
            let mut future = pin!(FixedLayout.route(request, &state));
            let _span = operation.measure_thread().iterations(1);
            match future.as_mut().poll(&mut context) {
                Poll::Ready(response) => response.status(),
                Poll::Pending => panic!("fixed route has no pending operation"),
            }
        } else {
            let request = Request::get("/").body(()).expect("valid request");
            let mut future = pin!(GenericApi.route(request, &state));
            let _span = operation.measure_thread().iterations(1);
            match future.as_mut().poll(&mut context) {
                Poll::Ready(response) => response.status(),
                Poll::Pending => panic!("generic route has no pending operation"),
            }
        };
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(total_bytes_allocated(&session, name), 0, "{name}");
    }
}

fn app_state() -> AppState {
    AppState {
        label: Arc::from("required"),
        sequence: 41,
        body_calls: Rc::new(Cell::new(0)),
    }
}

fn vector_request(method: &str, path: &str) -> Request<Vec<u8>> {
    Request::builder()
        .method(method)
        .uri(path)
        .body(Vec::new())
        .expect("test request metadata is valid")
}

async fn body<B>(response: Response<B>) -> Bytes
where
    B: http_body::Body<Data = Bytes>,
    B::Error: core::fmt::Debug,
{
    response.into_body().collect().await.expect("body succeeds").to_bytes()
}
