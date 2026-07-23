// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A [`tower_service::Service`] adapter for Routerama routing functions.
//!
//! [`RouteService`] stores a router, state, and function. Each request receives
//! owned clones because a Tower future cannot borrow the service. Routing
//! failures are responses, so the service error is [`Infallible`] and
//! readiness is always ready.
//!
//! A generated `#[router(..., tower)]` service exposes a `tower_service`
//! constructor that keeps its private concrete response body behind the
//! returned service's opaque associated response type. It is allocation-free
//! and enforces `Send + 'static` transport bounds without exporting handler or
//! rejection types.
//!
//! The general adapter's default [`ExactBody`] boundary preserves the routing response.
//! [`SendBoxedBody`] and [`LocalBoxedBody`] each allocate once to normalize the
//! response body. The adapter otherwise adds no trait object or boxed future;
//! auto traits follow its stored values and returned future.
//!
//! [`Infallible`]: core::convert::Infallible

use core::convert::Infallible;
use core::fmt;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

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
/// This is [`RouteService`]'s default boundary and allocates nothing. Use it
/// whenever the surrounding Tower stack does not need to name the response
/// body. For a nameable, transport-ready exact body, opt into the generated
/// `#[router(..., tower)]` constructor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExactBody;

/// Erases the response body once through [`SendBoxBody`].
///
/// This boundary allocates once and requires a `Send + 'static` body with a
/// `Send + Sync + 'static` error. Use [`LocalBoxedBody`] for local bodies.
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

impl<B, D> NormalizeResponse<Response<B>> for SendBoxedBody
where
    B: http_body::Body<Data = D> + Send + 'static,
    B::Error: core::error::Error + Send + Sync + 'static,
    D: bytes::Buf,
{
    type Response = Response<SendBoxBody<D>>;

    fn normalize(response: Response<B>) -> Self::Response {
        response.map(SendBoxBody::new)
    }
}

impl<B, D> NormalizeResponse<Response<B>> for LocalBoxedBody
where
    B: http_body::Body<Data = D> + 'static,
    B::Error: core::error::Error + 'static,
    D: bytes::Buf,
{
    type Response = Response<BoxBody<D>>;

    fn normalize(response: Response<B>) -> Self::Response {
        response.map(BoxBody::new)
    }
}

/// A [`tower_service::Service`] over an owned router, state, and routing call.
///
/// Select an exact, send-boxed, local-boxed, or custom response boundary.
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
    /// incoming request. Annotate the request parameter when its body type
    /// cannot be inferred.
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
    /// The response type becomes `http::Response<SendBoxBody<D>>` at the cost
    /// of one allocation per response. Prefer a generated
    /// `#[router(..., tower)]` constructor for a closed all-`Send` service; use
    /// this open erasure boundary when that generated adapter is unavailable.
    #[must_use]
    pub fn send_boxed_body(self) -> RouteService<Service, State, Call, SendBoxedBody> {
        self.normalized()
    }

    /// Selects the [`LocalBoxedBody`] response boundary.
    ///
    /// The response type becomes `http::Response<BoxBody<D>>` at the cost of
    /// one allocation per response, without requiring [`Send`]. Use this for
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
