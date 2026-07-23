// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The Tower transport adapter over generated, configured, and mounted routers.

#![cfg(feature = "tower")]

use std::pin::Pin;
#[cfg(not(miri))]
use std::pin::pin;
#[cfg(feature = "mount")]
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

#[cfg(not(miri))]
use alloc_tracker::{Allocator, Session};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::BodyExt as _;
use routerama::response::{Body, BoxBody, IntoResponse, Response, SendBoxBody};
use routerama::route::tower::{ExactBody, LocalBoxedBody, NormalizeResponse, RouteFuture, RouteService, SendBoxedBody};
use routerama::route::{Before, BeforeContext, ExtensionRef, Request, State, StatusCode, router};
use tower::ServiceBuilder;
use tower::limit::ConcurrencyLimitLayer;
use tower::util::MapRequestLayer;
use tower_service::Service as _;

#[cfg(not(miri))]
#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[derive(Clone)]
struct AppState {
    deployment: &'static str,
    calls: Arc<AtomicUsize>,
}

fn state() -> AppState {
    AppState {
        deployment: "west",
        calls: Arc::new(AtomicUsize::new(0)),
    }
}

/// A caller identity that a Tower layer attaches to the request.
#[derive(Clone, Copy)]
struct Caller(&'static str);

/// The identity a router-wide `#[before]` guard promotes for handlers.
#[derive(Clone, Copy)]
struct Authenticated(&'static str);

/// A zero-sized router: cloning it into each Tower call costs nothing.
#[derive(Clone, Copy)]
struct Api;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl Api {
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/books/{id}")]
    async fn book(&self, id: u32, state: State<AppState>) -> String {
        let _ = state.calls.fetch_add(1, Ordering::Relaxed);
        format!("{}:{id}", state.deployment)
    }

    #[route(GET, "/stream")]
    async fn stream(&self) -> Response<TrailerStream> {
        Response::new(TrailerStream { frame: 0, fail: false })
    }

    #[route(GET, "/broken")]
    async fn broken(&self) -> Response<TrailerStream> {
        Response::new(TrailerStream { frame: 0, fail: true })
    }
}

struct Guarded;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = ())]
impl Guarded {
    #[route(GET, "/whoami")]
    async fn whoami(&self, caller: ExtensionRef<'_, Authenticated>) -> String {
        caller.0.0.to_owned()
    }

    #[before]
    async fn authenticate(&self, context: &mut BeforeContext<'_>) -> Before<StatusCode> {
        match context.get_extension::<Caller>().copied() {
            Some(caller) => {
                let _ = context.insert_extension(Authenticated(caller.0));
                Before::Next
            }
            None => Before::Respond(StatusCode::UNAUTHORIZED),
        }
    }
}

struct Plugins;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl Plugins {
    #[route(GET, "/health")]
    async fn health(&self) -> &'static str {
        "static"
    }

    #[route(dynamic)]
    async fn plugin(&self, #[capture] name: String, state: State<AppState>) -> String {
        format!("{}:{name}", state.deployment)
    }
}

/// A multi-frame streaming body with trailers, optionally failing mid-stream.
struct TrailerStream {
    frame: u8,
    fail: bool,
}

#[derive(Debug)]
struct StreamError;

impl core::fmt::Display for StreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("the stream failed")
    }
}

impl core::error::Error for StreamError {}

impl HttpBody for TrailerStream {
    type Data = Bytes;
    type Error = StreamError;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let frame = self.frame;
        self.frame = frame.saturating_add(1);
        Poll::Ready(match (frame, self.fail) {
            (0, _) => Some(Ok(Frame::data(Bytes::from_static(b"first")))),
            (1, true) => Some(Err(StreamError)),
            (1, false) => Some(Ok(Frame::data(Bytes::from_static(b"second")))),
            (2, false) => {
                let mut trailers = HeaderMap::new();
                let _ = trailers.insert(HeaderName::from_static("x-checksum"), HeaderValue::from_static("ok"));
                Some(Ok(Frame::trailers(trailers)))
            }
            _ => None,
        })
    }

    fn is_end_stream(&self) -> bool {
        self.frame > 2
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(11)
    }
}

fn request(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).expect("the test request is valid")
}

async fn text<B>(response: Response<B>) -> Bytes
where
    B: HttpBody<Data = Bytes>,
    B::Error: core::fmt::Debug,
{
    response.into_body().collect().await.expect("the response body succeeds").to_bytes()
}

async fn ready<S, R>(service: &mut S) -> &mut S
where
    S: tower_service::Service<R, Error = core::convert::Infallible>,
{
    core::future::poll_fn(|context| service.poll_ready(context))
        .await
        .expect("routing readiness is infallible");
    service
}

#[tokio::test(flavor = "current_thread")]
async fn generated_static_router_serves_requests_through_tower() {
    let state = state();
    let mut service = RouteService::new(
        Arc::new(Api),
        state.clone(),
        |api: Arc<Api>, state: AppState, request: Request<Body>| async move { api.route(request, &state).await },
    );

    let response = ready(&mut service)
        .await
        .call(request("/books/42"))
        .await
        .expect("routing is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text(response).await, b"west:42"[..]);

    let missing = ready(&mut service)
        .await
        .call(request("/nope"))
        .await
        .expect("routing is infallible");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(state.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn configured_dynamic_and_mixed_routers_compose_with_one_shared_handle() {
    let router = Plugins::router_builder()
        .add_plugin("GET", "/plugins/{name}")
        .build()
        .expect("the dynamic route is valid");
    let mut service = RouteService::new(
        Arc::new((router, Plugins)),
        state(),
        |shared: Arc<(PluginsRouter, Plugins)>, state: AppState, request: Request<Body>| async move {
            shared.0.route(&shared.1, request, &state).await
        },
    )
    .send_boxed_body();

    let statically_routed = ready(&mut service)
        .await
        .call(request("/health"))
        .await
        .expect("routing is infallible");
    assert_eq!(text(statically_routed).await, b"static"[..]);

    let dynamically_routed = ready(&mut service)
        .await
        .call(request("/plugins/tracing"))
        .await
        .expect("routing is infallible");
    assert_eq!(text(dynamically_routed).await, b"west:tracing"[..]);
}

#[cfg(feature = "mount")]
#[tokio::test(flavor = "current_thread")]
async fn erased_mounts_compose_through_the_local_boxing_boundary() {
    use routerama::route::mount::{ErasedMountRouter, ErasedMountService, MountedRequest};

    let mounts = ErasedMountRouter::builder()
        .mount(
            "GET",
            "/plugins/{name}",
            ErasedMountService::<Body, AppState>::from_async_fn(async |request: MountedRequest<'_, Body>, state: &AppState| {
                let name = request.decoded_capture("name").expect("the template captures `name`");
                format!("{}:{name}", state.deployment)
            }),
        )
        .build()
        .expect("the mount is valid");

    // The mount core is deliberately local, so its shared handle is an `Rc`.
    let mut standalone = RouteService::new(
        Rc::new(mounts),
        state(),
        |mounts: Rc<ErasedMountRouter<Body, AppState>>, state: AppState, request: Request<Body>| async move {
            mounts.route(request, &state).await
        },
    );
    let response = ready(&mut standalone)
        .await
        .call(request("/plugins/search"))
        .await
        .expect("routing is infallible");
    assert_eq!(text(response).await, b"west:search"[..]);

    let miss = ready(&mut standalone)
        .await
        .call(request("/missing"))
        .await
        .expect("routing is infallible");
    assert_eq!(miss.status(), StatusCode::NOT_FOUND);
}

#[cfg(feature = "mount")]
#[tokio::test(flavor = "current_thread")]
async fn generated_static_first_mount_integration_names_one_boxed_response() {
    use routerama::route::mount::{ErasedMountRouter, ErasedMountService, MountedRequest};

    struct Mixed;

    #[allow(
        clippy::allow_attributes,
        unknown_lints,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
    )]
    #[router(state = AppState, erased_mounts)]
    impl Mixed {
        #[route(GET, "/health")]
        async fn health(&self) -> &'static str {
            "generated"
        }
    }

    let mounts = Rc::new(
        ErasedMountRouter::builder()
            .mount(
                "GET",
                "/plugins/{name}",
                ErasedMountService::<Body, AppState>::from_async_fn(
                    async |_request: MountedRequest<'_, Body>, _state: &AppState| "mounted",
                ),
            )
            .build()
            .expect("the mount is valid"),
    );

    let mut service = RouteService::new(
        Rc::new((Mixed, mounts)),
        state(),
        |shared: Rc<(Mixed, Rc<ErasedMountRouter<Body, AppState>>)>, state: AppState, request: Request<Body>| async move {
            shared.0.route_with_erased_mounts(request, &state, &shared.1).await
        },
    )
    .local_boxed_body();

    // `local_boxed_body` makes the structural `EitherBody<generated, BoxBody>`
    // response nameable, which an `impl Trait` return type otherwise cannot be.
    let named: fn(Response<BoxBody>) -> Response<BoxBody> = core::convert::identity;

    let generated = named(
        ready(&mut service)
            .await
            .call(request("/health"))
            .await
            .expect("routing is infallible"),
    );
    assert_eq!(text(generated).await, b"generated"[..]);

    let mounted = named(
        ready(&mut service)
            .await
            .call(request("/plugins/search"))
            .await
            .expect("routing is infallible"),
    );
    assert_eq!(text(mounted).await, b"mounted"[..]);
}

#[tokio::test(flavor = "current_thread")]
async fn readiness_is_always_ready_and_layer_readiness_still_applies() {
    let mut bare = RouteService::new(Api, state(), |api: Api, state: AppState, request: Request<Body>| async move {
        api.route(request, &state).await
    });
    let mut context = Context::from_waker(Waker::noop());

    for _ in 0..3 {
        assert!(matches!(bare.poll_ready(&mut context), Poll::Ready(Ok(()))));
    }

    // One permit: the adapter is always ready, but a readiness-bearing layer
    // above it keeps its own contract instead of being masked.
    let mut limited = ServiceBuilder::new().layer(ConcurrencyLimitLayer::new(1)).service(bare);
    assert!(matches!(limited.poll_ready(&mut context), Poll::Ready(Ok(()))));
    let inflight = limited.call(request("/health"));
    assert!(matches!(limited.poll_ready(&mut context), Poll::Pending));

    assert_eq!(inflight.await.expect("routing is infallible").status(), StatusCode::NO_CONTENT);
    assert!(matches!(limited.poll_ready(&mut context), Poll::Ready(Ok(()))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cloned_services_share_one_router_and_dispatch_concurrently() {
    let state = state();
    let service = RouteService::new(
        Arc::new(Api),
        state.clone(),
        |api: Arc<Api>, state: AppState, request: Request<Body>| async move { api.route(request, &state).await },
    )
    .send_boxed_body();

    let tasks: Vec<_> = (0..16_u32)
        .map(|id| {
            let mut service = service.clone();
            tokio::spawn(async move {
                let response = ready(&mut service)
                    .await
                    .call(request(&format!("/books/{id}")))
                    .await
                    .expect("routing is infallible");
                text(response).await
            })
        })
        .collect();

    for (id, task) in tasks.into_iter().enumerate() {
        let body = task.await.expect("the routing task completes");
        assert_eq!(body, Bytes::from(format!("west:{id}")));
    }
    // Every clone observed the same shared state and the same single router.
    assert_eq!(state.calls.load(Ordering::Relaxed), 16);
    assert_eq!(service.state().calls.load(Ordering::Relaxed), 16);
    assert_eq!(Arc::strong_count(service.service()), 1);
}

#[test]
fn transport_flavor_is_send_sync_and_static_without_bounding_core_routing() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}
    const fn assert_static<T: 'static>() {}
    const fn assert_identity<T>()
    where
        ExactBody: NormalizeResponse<T, Response = T>,
    {
    }
    const fn assert_local_boxed<T>()
    where
        LocalBoxedBody: NormalizeResponse<T, Response = Response<BoxBody>>,
    {
    }

    type Callable = fn(Arc<Api>, AppState, Request<Body>) -> core::future::Ready<Response<Body>>;
    type Adapter = RouteService<Arc<Api>, AppState, Callable, SendBoxedBody>;

    assert_send::<Adapter>();
    assert_sync::<Adapter>();
    assert_static::<Adapter>();
    assert_send::<RouteFuture<core::future::Ready<Response<Body>>, SendBoxedBody>>();
    assert_send::<SendBoxBody>();
    assert_static::<SendBoxBody>();

    // The response boundary is the only place the transport's auto traits are
    // imposed; the identity boundary keeps whatever the router produced.
    assert_send::<<SendBoxedBody as NormalizeResponse<Response<Body>>>::Response>();
    assert_identity::<Response<Body>>();
    assert_local_boxed::<Response<Body>>();
}

#[tokio::test(flavor = "current_thread")]
async fn erased_responses_preserve_frames_trailers_size_hints_and_errors() {
    let mut service = RouteService::new(
        Arc::new(Api),
        state(),
        |api: Arc<Api>, state: AppState, request: Request<Body>| async move { api.route(request, &state).await },
    )
    .send_boxed_body();

    let response = ready(&mut service)
        .await
        .call(request("/stream"))
        .await
        .expect("routing is infallible");
    let mut body = response.into_body();
    assert_eq!(body.size_hint().exact(), Some(11));
    assert!(!body.is_end_stream());

    let mut data = Vec::new();
    let mut trailers = None;
    while let Some(frame) = body.frame().await {
        let frame = frame.expect("the successful stream has no error frame");
        match frame.into_data() {
            Ok(chunk) => data.extend_from_slice(&chunk),
            Err(frame) => trailers = frame.into_trailers().ok(),
        }
    }
    assert_eq!(data, b"firstsecond");
    assert_eq!(trailers.expect("the stream ends with trailers")["x-checksum"], "ok");

    let failing = ready(&mut service)
        .await
        .call(request("/broken"))
        .await
        .expect("routing is infallible");
    let mut failing = failing.into_body();
    let first = failing
        .frame()
        .await
        .expect("the first frame arrives")
        .expect("the first frame succeeds");
    assert_eq!(
        first.into_data().expect("the first frame carries data"),
        Bytes::from_static(b"first")
    );
    let error = failing
        .frame()
        .await
        .expect("the second frame arrives")
        .expect_err("the stream fails on its second frame");
    // Erasure keeps the error `Send + Sync`, so it converts into the boxed
    // error Hyper and Axum expect. The message it carries is the generated
    // response-body error sum's, which names the failing response source; the
    // sum itself is what stops the concrete type from being downcast here.
    assert_eq!(error.as_error().to_string(), error.to_string());
    assert!(error.to_string().contains("TrailerStream"), "{error}");
    let boxed: Box<dyn core::error::Error + Send + Sync> = error.into_inner();
    assert_eq!(
        boxed.to_string(),
        "response body from handler response `Response < TrailerStream >` failed"
    );

    // Erasing a concrete body directly keeps its exact error type.
    let mut direct = SendBoxBody::new(TrailerStream { frame: 1, fail: true });
    let direct = core::future::poll_fn(|context| Pin::new(&mut direct).poll_frame(context))
        .await
        .expect("the failing frame arrives")
        .expect_err("the stream fails");
    assert!(direct.as_error().is::<StreamError>());
    assert_eq!(direct.to_string(), "the stream failed");
}

#[tokio::test(flavor = "current_thread")]
async fn tower_layers_feed_generated_interceptors_and_handler_extensions() {
    let mut service = ServiceBuilder::new()
        .layer(MapRequestLayer::new(|mut request: Request<Body>| {
            let _ = request.extensions_mut().insert(Caller("docs"));
            request
        }))
        .service(
            RouteService::new(
                Arc::new(Guarded),
                (),
                |guarded: Arc<Guarded>, (): (), request: Request<Body>| async move { guarded.route(request, &()).await },
            )
            .send_boxed_body(),
        );

    let authenticated = ready(&mut service)
        .await
        .call(request("/whoami"))
        .await
        .expect("routing is infallible");
    assert_eq!(text(authenticated).await, b"docs"[..]);

    // Without the layer, the router-wide `#[before]` guard short-circuits.
    let mut unguarded = RouteService::new(
        Arc::new(Guarded),
        (),
        |guarded: Arc<Guarded>, (): (), request: Request<Body>| async move { guarded.route(request, &()).await },
    )
    .send_boxed_body();
    let rejected = ready(&mut unguarded)
        .await
        .call(request("/whoami"))
        .await
        .expect("routing is infallible");
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
}

#[test]
#[cfg(not(miri))]
fn the_adapter_adds_no_allocation_and_body_erasure_costs_exactly_one() {
    let session = Session::new().no_stdout().no_file();
    let mut context = Context::from_waker(Waker::noop());

    let mut exact = RouteService::new(
        Arc::new(Api),
        state(),
        |api: Arc<Api>, state: AppState, request: Request<Body>| async move { api.route(request, &state).await },
    );
    let exact_operation = session.operation("tower_exact_body");
    let mut call = pin!(exact.call(request("/health")));
    let span = exact_operation.measure_thread();
    let response = match call.as_mut().poll(&mut context) {
        Poll::Ready(response) => response.expect("routing is infallible"),
        Poll::Pending => panic!("the static handler has no pending operation"),
    };
    drop(span);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(exact_operation.total_bytes_allocated(), 0);

    let mut erased = exact.send_boxed_body();
    let erased_operation = session.operation("tower_send_boxed_body");
    let mut call = pin!(erased.call(request("/health")));
    let span = erased_operation.measure_thread();
    let response = match call.as_mut().poll(&mut context) {
        Poll::Ready(response) => response.expect("routing is infallible"),
        Poll::Pending => panic!("the static handler has no pending operation"),
    };
    drop(span);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    drop((exact_operation, erased_operation));
    let report = session.to_report();
    let allocations = |name| {
        report
            .operations()
            .find(|(operation_name, _)| *operation_name == name)
            .expect("the measured operation is present")
            .1
            .total_allocations_count()
    };
    assert_eq!(allocations("tower_exact_body"), 0);
    assert_eq!(allocations("tower_send_boxed_body"), 1);
}

#[test]
#[cfg(not(miri))]
fn reapplying_the_send_boundary_nests_erasure_and_allocates_again() {
    let response = StatusCode::NO_CONTENT.into_response().map(SendBoxBody::new);
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("tower_repeated_send_boxing");
    let span = operation.measure_thread();
    let normalized = <SendBoxedBody as NormalizeResponse<Response<SendBoxBody>>>::normalize(response);
    drop(span);

    assert_eq!(normalized.status(), StatusCode::NO_CONTENT);
    drop(operation);
    let report = session.to_report();
    let allocations = report
        .operations()
        .find(|(name, _)| *name == "tower_repeated_send_boxing")
        .expect("the measured operation is present")
        .1
        .total_allocations_count();
    assert_eq!(allocations, 1);
}
