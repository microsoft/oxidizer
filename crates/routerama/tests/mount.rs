// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit runtime-mounted service behavior and generated static-path isolation.

#![cfg(feature = "mount")]

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::mount::{
    ErasedMountRouter, ErasedMountService, MountedCaptureError, MountedRequest, MountedService, SendErasedMountRouter,
    SendErasedMountService,
};
use routerama::route::{Request, RouteFailure, State, router};

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
    label: &'static str,
    generated_calls: Rc<Cell<usize>>,
    mounted_calls: Rc<Cell<usize>>,
}

fn state() -> AppState {
    AppState {
        label: "shared",
        generated_calls: Rc::new(Cell::new(0)),
        mounted_calls: Rc::new(Cell::new(0)),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn standalone_mounts_expose_zero_copy_captures_state_metadata_and_aliases() {
    let state = state();
    let service = ErasedMountService::<Body, AppState>::from_async_fn(async |request: MountedRequest<'_, Body>, state: &AppState| {
        state.mounted_calls.set(state.mounted_calls.get() + 1);
        let id = request.capture::<u32>("id")?;
        let slug = request.decoded_capture("slug")?;
        assert_eq!(request.raw_capture("slug"), Some("rust%20book"));
        assert_eq!(request.captures().collect::<Vec<_>>(), [("id", "42"), ("slug", "rust%20book")]);
        assert_eq!(request.request().headers()["x-request"], "preserved");

        let mut headers = HeaderMap::new();
        headers.insert(HeaderName::from_static("x-mounted"), HeaderValue::from_static("yes"));
        Ok::<_, MountedCaptureError>((StatusCode::CREATED, headers, format!("{}:{id}:{slug}", state.label)))
    });
    let mounts = ErasedMountRouter::builder()
        .mount("POST", "/items/{id}/{slug}", service.clone())
        .mount("PUT", "/aliases/{id}/{slug}", service)
        .build()
        .expect("mounted aliases are valid");

    for (method, path) in [("POST", "/items/42/rust%20book"), ("PUT", "/aliases/42/rust%20book")] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("x-request", "preserved")
            .body(Body::empty())
            .expect("test request is valid");
        let response = mounts.route(request, &state).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()["x-mounted"], "yes");
        assert_eq!(
            response.into_body().collect().await.expect("body succeeds").to_bytes(),
            b"shared:42:rust book"[..]
        );
    }
    assert_eq!(state.mounted_calls.get(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn standalone_miss_and_capture_failures_have_complete_http_policy() {
    let service = ErasedMountService::<Body, ()>::from_async_fn(async |request: MountedRequest<'_, Body>, _state: &()| {
        request.capture::<u32>("id").map(|id| format!("{id}"))
    });
    let mounts = ErasedMountRouter::builder()
        .mount(http::Method::GET, "/items/{id}", service)
        .build()
        .expect("mount is valid");

    let missing = mounts
        .route(Request::get("/missing").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let invalid = mounts
        .route(Request::get("/items/not-a-number").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let undecodable = mounts
        .route(Request::get("/items/%2").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(undecodable.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "current_thread")]
async fn affix_and_rest_capture_ranges_are_materialized_without_reparsing() {
    let inspect = ErasedMountService::<Body, ()>::from_async_fn(async |request: MountedRequest<'_, Body>, _state: &()| {
        request
            .captures()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(",")
    });
    let mounts = ErasedMountRouter::builder()
        .mount("GET", "/img-{id}.png", inspect.clone())
        .mount("GET", "/files/{tail=**}", inspect)
        .build()
        .expect("affix and rest mounts are valid");

    let affix = mounts
        .route(Request::get("/img-ferris.png").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(
        affix.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"id=ferris"[..]
    );

    let rest = mounts
        .route(Request::get("/files/a/b/c").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(
        rest.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"tail=a/b/c"[..]
    );
}

#[test]
fn mount_build_accumulates_invalid_methods_templates_and_deterministic_conflicts() {
    let service =
        ErasedMountService::<Body, ()>::from_async_fn(async |_request: MountedRequest<'_, Body>, _state: &()| StatusCode::NO_CONTENT);
    let error = ErasedMountRouter::builder()
        .mount("BAD METHOD", "/invalid-method", service.clone())
        .mount("GET", "missing-leading-slash", service.clone())
        .mount("GET", "/same/{first}", service.clone())
        .mount("GET", "/same/{second}", service)
        .build()
        .expect_err("every invalid registration must be rejected");
    let message = error.to_string();
    assert!(message.starts_with("failed to build erased mount router:"), "{message}");
    assert!(message.contains("BAD METHOD"), "{message}");
    assert!(message.contains("missing-leading-slash"), "{message}");
    assert!(message.contains("conflicting routes"), "{message}");
    assert!(message.contains("Mount0, Mount1"), "{message}");
    assert_eq!(error.causes().count(), 1);
}

/// Sibling literals per node in the wide-table tests, chosen to exceed the
/// runtime matcher's linear-scan threshold several times over.
const WIDE_TABLE_ENTRIES: usize = 256;

#[tokio::test(flavor = "current_thread")]
async fn wide_mount_tables_resolve_every_entry_and_keep_alias_and_wildcard_precedence() {
    let state = state();
    let service = ErasedMountService::<Body, AppState>::from_async_fn(async |request: MountedRequest<'_, Body>, state: &AppState| {
        state.mounted_calls.set(state.mounted_calls.get() + 1);
        let name = request.raw_capture("name").unwrap_or("literal");
        (StatusCode::OK, format!("{}:{name}", state.label))
    });
    let mut builder = ErasedMountRouter::builder();
    for entry in 0..WIDE_TABLE_ENTRIES {
        builder = builder.mount("GET", format!("/wide/entry-{entry:04}"), service.clone());
        // Every entry is also reachable through a second, differently ordered
        // alias table, so aliasing is unaffected by the sorted lookup.
        builder = builder.mount("GET", format!("/alias/{:04}-entry", WIDE_TABLE_ENTRIES - entry), service.clone());
    }
    let mounts = builder
        .mount("GET", "/wide/{name}", service.clone())
        .mount("GET", "/wide/{tail=**}", service)
        .build()
        .expect("a wide mount table is valid");

    for entry in [0, WIDE_TABLE_ENTRIES / 2, WIDE_TABLE_ENTRIES - 1] {
        for path in [
            format!("/wide/entry-{entry:04}"),
            format!("/alias/{:04}-entry", WIDE_TABLE_ENTRIES - entry),
        ] {
            let response = mounts
                .route(Request::get(&path).body(Body::empty()).expect("valid request"), &state)
                .await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                response.into_body().collect().await.expect("body succeeds").to_bytes(),
                b"shared:literal"[..],
                "{path} must select its literal entry, not the wildcard"
            );
        }
    }

    // A segment that is not a registered key falls to the single wildcard, and
    // a deeper path falls to the rest edge.
    let wildcard = mounts
        .route(Request::get("/wide/entry-9999").body(Body::empty()).expect("valid request"), &state)
        .await;
    assert_eq!(
        wildcard.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"shared:entry-9999"[..]
    );
    let rest = mounts
        .route(
            Request::get("/wide/entry-0000/deeper").body(Body::empty()).expect("valid request"),
            &state,
        )
        .await;
    assert_eq!(rest.status(), StatusCode::OK);

    let missing = mounts
        .route(
            Request::get("/alias/9999-entry").body(Body::empty()).expect("valid request"),
            &state,
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(state.mounted_calls.get(), 8);
}

#[test]
fn wide_mount_tables_still_report_deterministic_conflicts() {
    let service =
        ErasedMountService::<Body, ()>::from_async_fn(async |_request: MountedRequest<'_, Body>, _state: &()| StatusCode::NO_CONTENT);
    let mut builder = ErasedMountRouter::builder();
    for entry in 0..WIDE_TABLE_ENTRIES {
        builder = builder.mount("GET", format!("/wide/entry-{entry:04}"), service.clone());
    }
    let error = builder
        .mount("GET", "/wide/entry-0007", service.clone())
        .mount("GET", "/wide/{first}/leaf", service.clone())
        .mount("GET", "/wide/{second}/leaf", service)
        .build()
        .expect_err("a duplicated key and a wildcard conflict must both be rejected");
    let message = error.to_string();
    assert!(message.contains("conflicting routes"), "{message}");
    assert!(message.contains("Mount7, Mount256"), "{message}");
    assert!(message.contains("Mount257, Mount258"), "{message}");
}

#[tokio::test(flavor = "current_thread")]
async fn generated_miss_transfers_original_parts_body_and_state_without_boxing_the_request() {
    let state = state();
    let service = ErasedMountService::<Vec<u8>, AppState>::from_async_fn(async |request: MountedRequest<'_, Vec<u8>>, state: &AppState| {
        let id = request.capture::<u32>("id").expect("typed capture is valid");
        let (parts, mut body) = request.into_request().into_parts();
        assert_eq!(parts.headers["x-original"], "yes");
        assert_eq!(parts.extensions.get::<u32>(), Some(&73));
        body.extend_from_slice(format!(":{}:{id}", state.label).as_bytes());
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .header("x-body-owned", "yes")
            .body(Body::from(body))
            .expect("static response metadata is valid")
    });
    let mounts = ErasedMountRouter::builder()
        .mount("POST", "/echo/{id}", service)
        .build()
        .expect("mount is valid");
    let mut request = Request::post("/echo/9")
        .header("x-original", "yes")
        .body(b"payload".to_vec())
        .expect("valid request");
    request.extensions_mut().insert(73_u32);

    let response = StaticApi.route_with_erased_mounts(request, &state, &mounts).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["x-body-owned"], "yes");
    assert_eq!(
        response.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"payload:shared:9"[..]
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamError(&'static str);

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for StreamError {}

struct LocalStreamBody {
    frames: VecDeque<Result<Frame<Bytes>, StreamError>>,
    _local: Rc<()>,
}

impl LocalStreamBody {
    fn successful() -> Self {
        let mut trailers = HeaderMap::new();
        trailers.insert(HeaderName::from_static("x-trailer"), HeaderValue::from_static("complete"));
        Self {
            frames: [
                Ok(Frame::data(Bytes::from_static(b"first"))),
                Ok(Frame::data(Bytes::from_static(b"second"))),
                Ok(Frame::trailers(trailers)),
            ]
            .into(),
            _local: Rc::new(()),
        }
    }

    fn failing() -> Self {
        Self {
            frames: [Err(StreamError("mounted stream failed"))].into(),
            _local: Rc::new(()),
        }
    }
}

impl HttpBody for LocalStreamBody {
    type Data = Bytes;
    type Error = StreamError;

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

struct NamedStreamService;

impl MountedService<Body, AppState> for NamedStreamService {
    type Response = Response<LocalStreamBody>;

    #[expect(
        clippy::future_not_send,
        reason = "the mounted-service core deliberately supports local state and bodies"
    )]
    async fn call<'a>(&'a self, request: MountedRequest<'a, Body>, state: &'a AppState) -> Self::Response
    where
        Body: 'a,
    {
        assert_eq!(request.raw_capture("kind"), Some("success"));
        assert_eq!(state.label, "shared");
        core::future::ready(()).await;
        Response::new(LocalStreamBody::successful())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn named_and_local_async_services_preserve_stream_frames_trailers_and_errors() {
    let state = state();
    let local_marker = Rc::new(Cell::new(0));
    let failing_marker = Rc::clone(&local_marker);
    let successful = ErasedMountService::new(NamedStreamService);
    let failing =
        ErasedMountService::<Body, AppState>::from_async_fn(async move |_request: MountedRequest<'_, Body>, _state: &AppState| {
            failing_marker.set(failing_marker.get() + 1);
            core::future::ready(()).await;
            Response::new(LocalStreamBody::failing())
        });
    let mounts = ErasedMountRouter::builder()
        .mount("GET", "/stream/{kind}", successful)
        .mount("GET", "/failure", failing)
        .build()
        .expect("stream mounts are valid");

    let response = mounts
        .route(Request::get("/stream/success").body(Body::empty()).expect("valid request"), &state)
        .await;
    let mut body = response.into_body();
    assert_eq!(
        body.frame()
            .await
            .expect("first frame exists")
            .expect("first frame succeeds")
            .into_data()
            .expect("first frame is data"),
        b"first"[..]
    );
    assert_eq!(
        body.frame()
            .await
            .expect("second frame exists")
            .expect("second frame succeeds")
            .into_data()
            .expect("second frame is data"),
        b"second"[..]
    );
    let trailers = body
        .frame()
        .await
        .expect("trailer frame exists")
        .expect("trailer frame succeeds")
        .into_trailers()
        .expect("third frame contains trailers");
    assert_eq!(trailers["x-trailer"], "complete");

    let response = mounts
        .route(Request::get("/failure").body(Body::empty()).expect("valid request"), &state)
        .await;
    let error = response
        .into_body()
        .frame()
        .await
        .expect("error frame exists")
        .expect_err("stream fails");
    assert_eq!(error.to_string(), "mounted stream failed");
    assert!(error.as_error().is::<StreamError>());
    assert_eq!(local_marker.get(), 1);
}

struct StaticApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::future_not_send,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async and the test state deliberately contains Rc; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl StaticApi {
    #[route(GET, "/static")]
    async fn static_route(&self, state: State<AppState>) -> StatusCode {
        state.generated_calls.set(state.generated_calls.get() + 1);
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/generated/{id}")]
    async fn generated_capture(&self, id: u32) -> String {
        format!("generated:{id}")
    }

    #[route(GET, "/borrowed/{name}")]
    async fn borrowed_capture(&self, name: &str) -> String {
        format!("borrowed:{name}")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn generated_static_routes_win_and_only_complete_misses_reach_mounts() {
    let state = state();
    let service = ErasedMountService::<Body, AppState>::from_async_fn(async |request: MountedRequest<'_, Body>, state: &AppState| {
        state.mounted_calls.set(state.mounted_calls.get() + 1);
        format!("mounted:{}", request.request().uri().path())
    });
    let mounts = ErasedMountRouter::builder()
        .mount("GET", "/static", service.clone())
        .mount("GET", "/generated/{id}", service.clone())
        .mount("GET", "/mounted", service)
        .build()
        .expect("mounts are valid internally");

    let static_response = StaticApi
        .route_with_erased_mounts(Request::get("/static").body(Body::empty()).expect("valid request"), &state, &mounts)
        .await;
    assert_eq!(static_response.status(), StatusCode::NO_CONTENT);
    assert_eq!(state.generated_calls.get(), 1);
    assert_eq!(state.mounted_calls.get(), 0);

    let borrowed = StaticApi
        .route_with_erased_mounts(
            Request::get("/borrowed/ferris").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(
        borrowed.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"borrowed:ferris"[..]
    );
    assert_eq!(state.mounted_calls.get(), 0);

    let capture_failure = StaticApi
        .route_with_erased_mounts(
            Request::get("/generated/not-a-number").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(capture_failure.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.mounted_calls.get(), 0);

    let mounted = StaticApi
        .route_with_erased_mounts(
            Request::get("/mounted").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(
        mounted.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"mounted:/mounted"[..]
    );
    assert_eq!(state.mounted_calls.get(), 1);

    let missing = StaticApi
        .route_with_erased_mounts(
            Request::get("/missing").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(state.mounted_calls.get(), 1);
}

struct MountFallbackApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl MountFallbackApi {
    #[route(GET, "/generated")]
    async fn generated(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[fallback]
    async fn fallback(&self, _failure: RouteFailure<'_>) -> StatusCode {
        StatusCode::IM_A_TEAPOT
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mount_miss_is_the_final_backstop_and_does_not_invoke_generated_fallback() {
    let state = state();
    let mounts = ErasedMountRouter::<Body, AppState>::builder()
        .build()
        .expect("empty mount table is valid");

    let ordinary = MountFallbackApi
        .route(Request::get("/missing").body(Body::empty()).expect("valid request"), &state)
        .await;
    assert_eq!(ordinary.status(), StatusCode::IM_A_TEAPOT);

    let mounted = MountFallbackApi
        .route_with_erased_mounts(
            Request::get("/missing").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(mounted.status(), StatusCode::NOT_FOUND);
}

struct DynamicOnlyApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl DynamicOnlyApi {
    #[route(dynamic)]
    async fn configured(&self, #[capture] name: String) -> String {
        format!("configured:{name}")
    }
}

struct MixedApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState, erased_mounts)]
impl MixedApi {
    #[route(GET, "/fixed")]
    async fn fixed(&self) -> &'static str {
        "fixed"
    }

    #[route(dynamic)]
    async fn configured(&self) -> &'static str {
        "configured"
    }
}

#[tokio::test(flavor = "current_thread")]
async fn generated_dynamic_only_and_mixed_routers_precede_mounted_fallbacks() {
    let state = state();
    let service =
        ErasedMountService::<Body, AppState>::from_async_fn(async |_request: MountedRequest<'_, Body>, _state: &AppState| "mounted");
    let mounts = ErasedMountRouter::builder()
        .mount("GET", "/configured/plugin", service.clone())
        .mount("GET", "/configured", service.clone())
        .mount("GET", "/mounted", service)
        .build()
        .expect("mounts are valid");

    let dynamic = DynamicOnlyApi::router_builder()
        .add_configured("GET", "/configured/{name}")
        .build()
        .expect("configured dynamic route is valid");
    let response = dynamic
        .route_with_erased_mounts(
            &DynamicOnlyApi,
            Request::get("/configured/plugin").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(
        response.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"configured:plugin"[..]
    );
    let response = dynamic
        .route_with_erased_mounts(
            &DynamicOnlyApi,
            Request::get("/mounted").body(Body::empty()).expect("valid request"),
            &state,
            &mounts,
        )
        .await;
    assert_eq!(
        response.into_body().collect().await.expect("body succeeds").to_bytes(),
        b"mounted"[..]
    );

    let mixed = MixedApi::router_builder()
        .add_configured("GET", "/configured")
        .build()
        .expect("mixed router is valid");
    for (path, expected) in [
        ("/fixed", &b"fixed"[..]),
        ("/configured", &b"configured"[..]),
        ("/mounted", &b"mounted"[..]),
    ] {
        let response = mixed
            .route_with_erased_mounts(
                &MixedApi,
                Request::get(path).body(Body::empty()).expect("valid request"),
                &state,
                &mounts,
            )
            .await;
        assert_eq!(response.into_body().collect().await.expect("body succeeds").to_bytes(), expected);
    }
}

#[test]
#[cfg(not(miri))]
fn configured_mounts_cost_static_requests_no_allocations_or_erased_calls() {
    let state = state();
    let service = ErasedMountService::<Body, AppState>::from_async_fn(async |_request: MountedRequest<'_, Body>, state: &AppState| {
        state.mounted_calls.set(state.mounted_calls.get() + 1);
        StatusCode::NO_CONTENT
    });
    let mounts = ErasedMountRouter::builder()
        .mount("GET", "/mounted", service)
        .build()
        .expect("mount is valid");
    let session = Session::new().no_stdout().no_file();
    let static_operation = session.operation("generated_static_branch");
    let request = Request::get("/static").body(Body::empty()).expect("valid request");
    let mut future = pin!(StaticApi.route_with_erased_mounts(request, &state, &mounts));
    let mut context = Context::from_waker(Waker::noop());
    let static_span = static_operation.measure_thread().iterations(1);
    let response = match future.as_mut().poll(&mut context) {
        Poll::Ready(response) => response,
        Poll::Pending => panic!("static handler has no pending operation"),
    };
    drop(static_span);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(total_bytes_allocated(&session, "generated_static_branch"), 0);
    assert_eq!(state.generated_calls.get(), 1);
    assert_eq!(state.mounted_calls.get(), 0);

    let mounted_operation = session.operation("mounted_branch");
    let request = Request::get("/mounted").body(Body::empty()).expect("valid request");
    let mut future = pin!(StaticApi.route_with_erased_mounts(request, &state, &mounts));
    let mounted_span = mounted_operation.measure_thread().iterations(1);
    let response = match future.as_mut().poll(&mut context) {
        Poll::Ready(response) => response,
        Poll::Pending => panic!("mounted handler has no pending operation"),
    };
    drop(mounted_span);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(total_bytes_allocated(&session, "mounted_branch") > 0);
    assert_eq!(state.mounted_calls.get(), 1);

    let miss_operation = session.operation("standalone_mount_miss");
    let request = Request::get("/missing").body(Body::empty()).expect("valid request");
    let mut future = pin!(mounts.route(request, &state));
    let miss_span = miss_operation.measure_thread().iterations(1);
    let response = match future.as_mut().poll(&mut context) {
        Poll::Ready(response) => response,
        Poll::Pending => panic!("mount miss has no pending operation"),
    };
    drop(miss_span);
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(state.mounted_calls.get(), 1);

    drop((static_operation, mounted_operation, miss_operation));
    let report = session.to_report();
    let allocation_count = |name| {
        report
            .operations()
            .find(|(operation_name, _)| *operation_name == name)
            .expect("measured operation is present")
            .1
            .total_allocations_count()
    };
    assert_eq!(allocation_count("generated_static_branch"), 0);
    assert_eq!(allocation_count("mounted_branch"), 2);
    assert_eq!(allocation_count("standalone_mount_miss"), 1);
}

/// A `Send` state, since a `Send` mounted service borrows it across an await.
#[derive(Clone)]
struct SendState {
    label: &'static str,
}

struct SendGreeting;

impl routerama::route::mount::SendMountedService<Body, SendState> for SendGreeting {
    type Response = Result<String, MountedCaptureError>;

    async fn call<'a>(&'a self, request: MountedRequest<'a, Body>, state: &'a SendState) -> Self::Response
    where
        Body: 'a,
    {
        let id = request.capture::<u32>("id")?;
        core::future::ready(()).await;
        Ok(format!("{}:{id}", state.label))
    }
}

struct SendApi;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = SendState, erased_mounts)]
impl SendApi {
    #[route(GET, "/generated")]
    async fn generated(&self) -> &'static str {
        "generated"
    }
}

/// The generated mounted entry is generic over [`MountDelegate`], so the same
/// method serves the local and the `Send` mount router, and the `Send`ness of
/// its opaque return type follows the router that was passed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_mounts_reach_a_multi_threaded_transport_through_the_generated_entry() {
    let state = SendState { label: "shared" };
    let mounts = SendErasedMountRouter::<Body, SendState>::builder()
        .mount("GET", "/mounted/{id}", SendErasedMountService::new(SendGreeting))
        .build()
        .expect("the mount registration is valid");

    // Spawning requires the whole routing future to be `Send`, which is
    // precisely what the local `BoxBody` boundary cannot provide.
    let mounted = tokio::spawn(async move {
        let request = Request::get("/mounted/7").body(Body::empty()).expect("valid request");
        let response = SendApi.route_with_erased_mounts(request, &state, &mounts).await;
        assert_eq!(response.status(), StatusCode::OK);

        let generated = Request::get("/generated").body(Body::empty()).expect("valid request");
        let generated = SendApi.route_with_erased_mounts(generated, &state, &mounts).await;
        assert_eq!(generated.status(), StatusCode::OK);

        (
            response.into_body().collect().await.expect("the mounted body completes").to_bytes(),
            generated
                .into_body()
                .collect()
                .await
                .expect("the generated body completes")
                .to_bytes(),
        )
    })
    .await
    .expect("the spawned routing task completes");

    assert_eq!(mounted.0, Bytes::from_static(b"shared:7"));
    assert_eq!(mounted.1, Bytes::from_static(b"generated"));
}
