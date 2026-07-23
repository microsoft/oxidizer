// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A [`tower_service::Service`] adapter for generated and mounted routers.
//!
//! This module is the outer transport boundary described by the crate's
//! routing model. Core routing stays local, direct, and unboxed; the adapter
//! adds exactly the ownership and auto-trait properties a Tower stack needs and
//! nothing else. It is enabled by the additive `tower` Cargo feature, which
//! implies `route` and adds only the `tower-service` trait crate.
//!
//! # What it adapts
//!
//! [`RouteService`] wraps *any* callable that turns an owned
//! [`http::Request<B>`] into a response future. That covers every routing
//! entry Routerama generates or provides:
//!
//! - a generated static service's `route(request, &state)`;
//! - a configured dynamic or mixed `Router::route(&service, request, &state)`;
//! - the `mount` feature's `route_with_erased_mounts(request, &state, &mounts)`
//!   and a standalone `mount::ErasedMountRouter::route`; and
//! - any application function that composes these.
//!
//! Because the adapter never names a generated type, it needs no code
//! generation, no macro attribute, and no renamed-dependency lookup.
//!
//! ```
//! use std::sync::Arc;
//!
//! use routerama::response::{Body, Response, SendBoxBody};
//! use routerama::route::tower::RouteService;
//! use routerama::route::{Request, State, router};
//!
//! #[derive(Clone)]
//! struct AppState {
//!     deployment: &'static str,
//! }
//!
//! struct Api;
//!
//! #[router(state = AppState)]
//! impl Api {
//!     #[route(GET, "/books/{id}")]
//!     async fn book(&self, id: u32, state: State<AppState>) -> String {
//!         format!("{}:{id}", state.deployment)
//!     }
//! }
//!
//! fn service() -> impl tower_service::Service<
//!     Request<Body>,
//!     Response = Response<SendBoxBody>,
//!     Error = core::convert::Infallible,
//! > + Clone {
//!     RouteService::new(
//!         Arc::new(Api),
//!         Arc::new(AppState { deployment: "west" }),
//!         |api: Arc<Api>, state: Arc<AppState>, request: Request<Body>| async move {
//!             api.route(request, &state).await
//!         },
//!     )
//!     .send_boxed_body()
//! }
//! # let _ = service();
//! ```
//!
//! # Ownership
//!
//! [`tower_service::Service::call`] cannot return a future that borrows the
//! service, so the adapter hands the callable **owned** clones of the router
//! and state and the callable's future owns them for its whole lifetime. The
//! adapter stores the concrete `Service`, `State`, and callable types: it adds
//! no [`Arc`](alloc::sync::Arc), no trait object, and no per-call vtable of its
//! own. Applications choose their own sharing strategy — a zero-sized unit
//! router and a `Copy` state clone for free, while a large state can be shared
//! with `Arc` so a clone is one atomic increment.
//!
//! # Readiness
//!
//! Generated routing has no queue, connection pool, permit, or other
//! backpressure, so [`poll_ready`](tower_service::Service::poll_ready) is
//! honestly always `Poll::Ready(Ok(()))` and never returns an error. It is
//! never a lie about a deferred resource. Layers that *do* have readiness (for
//! example `tower::limit::ConcurrencyLimit`) compose above the adapter and keep
//! their own contract; the adapter neither swallows nor duplicates it.
//!
//! # Errors
//!
//! Routing itself cannot fail: every routing failure, predicate rejection,
//! extractor rejection, and mounted miss is already an HTTP response. The
//! service error type is therefore [`Infallible`], which is exactly what Hyper
//! and Axum want at the outermost layer. **Body** errors remain body errors and
//! surface while the response body is polled.
//!
//! # Auto traits and the response boundary
//!
//! The adapter imposes no [`Send`], [`Sync`], or `'static` bound of its own.
//! Auto traits flow structurally: `RouteService<Service, State, Call, _>` is
//! `Send`/`Sync` when its three stored values are, and [`RouteFuture`] is
//! `Send` when the callable's future is. A transport imposes its own bounds,
//! and the response-body boundary is the explicit knob for satisfying them:
//!
//! | Boundary | [`Service::Response`](tower_service::Service::Response) | Cost per response | Use for |
//! |---|---|---|---|
//! | [`ExactBody`] (default) | the callable's own response, unchanged | none | body-agnostic layers, benchmarks, tests |
//! | [`SendBoxedBody`] | `http::Response<SendBoxBody>` | one allocation | Hyper, Axum, and any `Send + 'static` transport |
//! | [`LocalBoxedBody`] | `http::Response<BoxBody>` | one allocation | single-threaded transports and erased mounts |
//!
//! A generated router's response body is a private concrete sum returned
//! behind an opaque type, so it cannot be *named* even though it is often
//! already `Send`. [`ExactBody`] still works wherever the response type never
//! has to be written down (`impl Service<..>`, `let` bindings, generic layers);
//! naming it in a struct field or a public signature is what requires an
//! erasure. Normalization happens exactly once, when the callable's future
//! completes, and it never touches the request.
//!
//! [`SendBoxedBody`] additionally requires the concrete body to be
//! `Send + 'static` with a `Send + Sync + 'static` error, which is where the
//! transport flavor's auto-trait requirements land. Erased mounts stay local by
//! design (their [`BoxBody`] is deliberately not `Send`), so a mounted router
//! composes through [`ExactBody`] or [`LocalBoxedBody`].
//!
//! # Cost summary
//!
//! Per request, over the underlying routing call, the adapter adds:
//!
//! - one clone of the stored router and one clone of the stored state;
//! - **no** boxed future — [`RouteFuture`] is a named, pin-projected wrapper
//!   holding the callable's own future inline;
//! - **no** service trait object, vtable call, or type map;
//! - **no** request boxing, cloning, or re-parsing; and
//! - exactly one response-body allocation *if* a boxing boundary is selected,
//!   and zero otherwise.
//!
//! `tests/tower.rs` pins these numbers with allocation counters: a generated
//! static route dispatched through [`ExactBody`] allocates zero bytes, and the
//! same route through [`SendBoxedBody`] allocates exactly one object.
//!
//! [`BoxBody`]: crate::response::BoxBody
//! [`SendBoxBody`]: crate::response::SendBoxBody

use core::convert::Infallible;
use core::fmt;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::Bytes;
use pin_project_lite::pin_project;

use crate::response::{BoxBody, Response, SendBoxBody};

/// The response boundary applied once to a completed routing call.
///
/// Implementations are markers, not values: they are selected as the last type
/// parameter of [`RouteService`] and decide that adapter's
/// [`Service::Response`](tower_service::Service::Response). The built-in
/// boundaries are [`ExactBody`], [`SendBoxedBody`], and [`LocalBoxedBody`];
/// implement this trait for your own marker to normalize into another
/// transport's body type instead.
pub trait NormalizeResponse<R> {
    /// The response this boundary produces.
    type Response;

    /// Converts one completed routing response.
    ///
    /// This runs exactly once per request, after the routing future resolves.
    fn normalize(response: R) -> Self::Response;
}

/// Keeps the router's own response type, adding no conversion at all.
///
/// This is [`RouteService`]'s default boundary and the only one that allocates
/// nothing. Use it whenever the surrounding Tower stack does not need to name
/// the response body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExactBody;

/// Erases the response body once through [`SendBoxBody`].
///
/// This is the transport boundary for Hyper, Axum, and any other stack that
/// requires a `Send + 'static` response body. It costs one allocation per
/// response and requires the router's concrete body to be `Send + 'static`
/// with a `Send + Sync + 'static` error.
///
/// The deliberately local [`BoxBody`] therefore cannot cross it; use
/// [`LocalBoxedBody`] instead:
///
/// ```compile_fail
/// use routerama::response::{Body, BoxBody, Response};
/// use routerama::route::Request;
/// use routerama::route::tower::RouteService;
///
/// let service = RouteService::new((), (), |(): (), (): (), request: Request<Body>| async move {
///     let _ = request;
///     Response::new(BoxBody::new(Body::empty()))
/// })
/// .send_boxed_body();
///
/// fn assert_service<S: tower_service::Service<Request<Body>>>(service: S) {
///     let _ = service;
/// }
/// assert_service(service);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendBoxedBody;

/// Erases the response body once through the local [`BoxBody`].
///
/// Use this for single-threaded transports and for erased mounts, whose
/// response bodies are deliberately not required to be [`Send`]. It costs one
/// allocation per response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalBoxedBody;

impl<R> NormalizeResponse<R> for ExactBody {
    type Response = R;

    fn normalize(response: R) -> Self::Response {
        response
    }
}

impl<B> NormalizeResponse<Response<B>> for SendBoxedBody
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: core::error::Error + Send + Sync + 'static,
{
    type Response = Response<SendBoxBody>;

    fn normalize(response: Response<B>) -> Self::Response {
        response.map(SendBoxBody::new)
    }
}

impl<B> NormalizeResponse<Response<B>> for LocalBoxedBody
where
    B: http_body::Body<Data = Bytes> + 'static,
    B::Error: core::error::Error + 'static,
{
    type Response = Response<BoxBody>;

    fn normalize(response: Response<B>) -> Self::Response {
        response.map(BoxBody::new)
    }
}

/// A [`tower_service::Service`] over an owned router, state, and routing call.
///
/// Construct it with [`new`](Self::new) and select a response boundary with
/// [`send_boxed_body`](Self::send_boxed_body),
/// [`local_boxed_body`](Self::local_boxed_body), or
/// [`normalized`](Self::normalized). See the [module
/// documentation](self) for the ownership, readiness, auto-trait, and cost
/// contracts.
///
/// The adapter is [`Clone`] whenever its three stored values are, which is what
/// Hyper's per-connection and Axum's per-request cloning require. Cloned
/// adapters share whatever the application shared: cloning an `Arc` router
/// keeps one router instance, while cloning a value router copies it.
pub struct RouteService<Service, State, Call, Boundary = ExactBody> {
    service: Service,
    state: State,
    call: Call,
    boundary: PhantomData<fn() -> Boundary>,
}

impl<Service, State, Call> RouteService<Service, State, Call> {
    /// Creates an adapter from a router, its shared state, and a routing call.
    ///
    /// `call` receives owned clones of `service` and `state` together with the
    /// incoming request, so the future it returns owns everything it needs.
    /// This is what lets the returned future satisfy
    /// [`tower_service::Service::Future`], which cannot borrow the service.
    ///
    /// The request-body type `B` is fixed by the callable. Annotate the
    /// closure's request parameter (or turbofish `new::<MyBody, _>`) when it
    /// cannot be inferred from the surrounding transport.
    ///
    /// ```
    /// use routerama::response::Body;
    /// use routerama::route::tower::RouteService;
    /// use routerama::route::{Request, StatusCode, router};
    ///
    /// #[derive(Clone, Copy)]
    /// struct Health;
    ///
    /// #[router(state = ())]
    /// impl Health {
    ///     #[route(GET, "/health")]
    ///     async fn health(&self) -> StatusCode {
    ///         StatusCode::NO_CONTENT
    ///     }
    /// }
    ///
    /// let service = RouteService::new(Health, (), |health: Health, (): (), request: Request<Body>| async move {
    ///     health.route(request, &()).await
    /// });
    /// # let _ = service;
    /// ```
    #[must_use]
    pub fn new<B, Fut>(service: Service, state: State, call: Call) -> Self
    where
        Service: Clone,
        State: Clone,
        Call: Fn(Service, State, http::Request<B>) -> Fut,
        Fut: Future,
    {
        Self {
            service,
            state,
            call,
            boundary: PhantomData,
        }
    }
}

impl<Service, State, Call, Boundary> RouteService<Service, State, Call, Boundary> {
    /// Borrows the stored router.
    pub const fn service(&self) -> &Service {
        &self.service
    }

    /// Borrows the stored shared state.
    pub const fn state(&self) -> &State {
        &self.state
    }

    /// Selects the [`SendBoxedBody`] response boundary.
    ///
    /// The response type becomes `http::Response<SendBoxBody>` at the cost of
    /// one allocation per response. Use this for Hyper, Axum, and other
    /// transports that require a `Send + 'static` response body.
    #[must_use]
    pub fn send_boxed_body(self) -> RouteService<Service, State, Call, SendBoxedBody> {
        self.normalized()
    }

    /// Selects the [`LocalBoxedBody`] response boundary.
    ///
    /// The response type becomes `http::Response<BoxBody>` at the cost of one
    /// allocation per response, without requiring [`Send`]. Use this for
    /// single-threaded transports and for erased mounts.
    #[must_use]
    pub fn local_boxed_body(self) -> RouteService<Service, State, Call, LocalBoxedBody> {
        self.normalized()
    }

    /// Selects an arbitrary [`NormalizeResponse`] boundary.
    ///
    /// This is the extension point for normalizing into another transport's
    /// body type without an intermediate erasure.
    #[must_use]
    pub fn normalized<Other>(self) -> RouteService<Service, State, Call, Other> {
        RouteService {
            service: self.service,
            state: self.state,
            call: self.call,
            boundary: PhantomData,
        }
    }
}

impl<Service, State, Call, Boundary> Clone for RouteService<Service, State, Call, Boundary>
where
    Service: Clone,
    State: Clone,
    Call: Clone,
{
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            state: self.state.clone(),
            call: self.call.clone(),
            boundary: PhantomData,
        }
    }
}

impl<Service, State, Call, Boundary> fmt::Debug for RouteService<Service, State, Call, Boundary> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteService").finish_non_exhaustive()
    }
}

impl<B, Service, State, Call, Fut, Boundary> tower_service::Service<http::Request<B>> for RouteService<Service, State, Call, Boundary>
where
    Service: Clone,
    State: Clone,
    Call: Fn(Service, State, http::Request<B>) -> Fut,
    Fut: Future,
    Boundary: NormalizeResponse<Fut::Output>,
{
    type Response = Boundary::Response;
    type Error = Infallible;
    type Future = RouteFuture<Fut, Boundary>;

    /// Always reports readiness.
    ///
    /// Generated routing owns no permit, queue, or connection, so there is
    /// nothing to wait for and nothing that can fail here.
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        RouteFuture {
            future: (self.call)(self.service.clone(), self.state.clone(), req),
            boundary: PhantomData,
        }
    }
}

pin_project! {
    /// The named future returned by [`RouteService`].
    ///
    /// It holds the routing future inline — nothing is boxed — and applies the
    /// selected [`NormalizeResponse`] boundary exactly once when that future
    /// completes. It is [`Send`] whenever the routing future is.
    pub struct RouteFuture<Fut, Boundary = ExactBody> {
        #[pin]
        future: Fut,
        boundary: PhantomData<fn() -> Boundary>,
    }
}

impl<Fut, Boundary> fmt::Debug for RouteFuture<Fut, Boundary> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteFuture").finish_non_exhaustive()
    }
}

impl<Fut, Boundary> Future for RouteFuture<Fut, Boundary>
where
    Fut: Future,
    Boundary: NormalizeResponse<Fut::Output>,
{
    type Output = Result<Boundary::Response, Infallible>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx).map(|response| Ok(Boundary::normalize(response)))
    }
}
