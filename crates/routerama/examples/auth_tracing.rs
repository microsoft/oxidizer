// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Authentication and `tracing` correlation on one generated router.
//!
//! Run it with `cargo run --example auth_tracing --features tower`.
//!
//! The `interceptors` example shows every interceptor *kind* (router-wide and
//! per-handler `#[before]`, buffering and streaming `#[transform]`, and
//! `#[after]`). This one instead shows the two cross-cutting concerns a real
//! service reaches for first — authentication and request tracing — composed
//! the performance-first way:
//!
//! ```text
//! Tower MapRequest   assigns a correlation id extension at the transport edge
//!   +- #[before]     opens the request span, authenticates, inserts a typed
//!   |                principal, or short-circuits with 401
//!   +- handler       borrows the principal and the path capture, and attaches
//!   |                the span to its own future
//!   +- #[after]      records the final status of every generated response
//! ```
//!
//! # What is borrowed and what is not
//!
//! The `#[before]` guard is the only place that touches the type map on the
//! request path: it inserts [`Principal`] and [`RequestSpan`]. The handler then
//! declares [`ExtensionRef`] parameters, so it receives `&Principal` and
//! `&RequestSpan` borrowed straight out of the request head — no clone and no
//! lookup beyond the extraction it asked for — alongside its zero-copy `&str`
//! path capture, which still borrows the request URI. A guard that has to run
//! *after* route selection keeps that property too, through
//! `SelectedContext`'s split request-head borrow; see the `interceptors`
//! example.
//!
//! # Exact `tracing` semantics
//!
//! Generated interceptors are ordinary `async` methods, so a `#[before]`
//! *returns* before the handler runs: an
//! [`Entered`](tracing::span::Entered) guard taken there would be dropped
//! immediately, and holding such a guard across an `await` is unsound practice
//! anyway because a suspended task leaves the thread with the span still
//! entered. There is therefore no transparent "enter the span for the whole
//! request" hook, and this example does not fake one. Instead:
//!
//! - the router-wide `#[before]` creates the span (with the correlation id,
//!   method, and path as fields) and stores it in the request extensions before
//!   it authenticates, so *every* outcome is correlated — including the `401`
//!   it may return one line later;
//! - synchronous emission sites use [`Span::in_scope`](tracing::Span::in_scope),
//!   which enters and exits inside one non-`async` closure;
//! - the handler attaches the span to its own future with
//!   [`Instrument::instrument`](tracing::Instrument::instrument), which is the
//!   safe way to keep a span current *across* `await` points: the span is
//!   entered on every poll and exited on every yield;
//! - the bare `#[after]` re-enters the same span through the immutable request
//!   head it borrows and emits one `http.response` event carrying the final
//!   status.
//!
//! Because a bare `#[after]` observes **every response this router generates**,
//! that one event covers handler responses, the `#[before]` short-circuit, and
//! routing failures alike. It does not observe a response produced by a
//! *mounted* service (whose request head is moved into the mount), so a service
//! that mounts must trace inside the mounted service too; see the
//! `mounted_services` example.
//!
//! The span itself closes when the request head is dropped, which happens after
//! the router returns the response, so a `tracing` layer that reports span
//! durations measures the whole generated request.

use core::convert::Infallible;
use core::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use http_body_util::BodyExt as _;
use routerama::response::{Body, SendBoxBody};
use routerama::route::tower::RouteService;
use routerama::route::{AfterContext, Before, BeforeContext, ExtensionRef, Request, StatusCode, router};
use tower::util::MapRequestLayer;
use tower::{ServiceBuilder, ServiceExt as _};
use tower_service::Service;
use tracing::{Instrument as _, Level, Span};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// The transport header that carries a caller-supplied correlation id.
const CORRELATION_HEADER: &str = "x-request-id";

/// The credentials this demo accepts, as `token -> display name`.
const ACCOUNTS: [(&str, &str); 2] = [("token-ada", "ada"), ("token-grace", "grace")];

/// Correlation ids assigned to requests that arrive without one.
static NEXT_CORRELATION: AtomicU64 = AtomicU64::new(1000);

/// The response type of the assembled Tower stack.
///
/// Naming it is the only reason this example boxes a response body: the
/// generated body is a private concrete sum, so `send_boxed_body()` pays one
/// allocation per response to make it writable in a signature. Routing that is
/// never written down keeps the unboxed body.
type BoxedResponse = routerama::response::Response<SendBoxBody>;

/// The correlation id a Tower layer attaches at the transport edge.
#[derive(Clone, Copy, Debug)]
struct CorrelationId(u64);

/// The authenticated caller the `#[before]` guard inserts.
///
/// Handlers borrow it through [`ExtensionRef`] rather than cloning it.
#[derive(Clone, Debug)]
struct Principal {
    id: u64,
    name: &'static str,
}

/// The request span, carried through the typed extensions.
///
/// A [`Span`] handle is cheap to clone and is `Send + Sync + 'static`, which is
/// exactly what [`http::Extensions`] requires.
#[derive(Clone, Debug)]
struct RequestSpan(Span);

/// A zero-sized router: cloning it into each Tower call costs nothing.
#[derive(Clone, Copy, Debug)]
struct Orders;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers and interceptors must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Orders {
    /// A liveness probe that deliberately skips authentication.
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    /// `id` is borrowed from the request URI and `principal` from the request
    /// extensions, so this handler allocates only for its response string.
    #[route(GET, "/orders/{id}")]
    async fn order(&self, id: &str, principal: ExtensionRef<'_, Principal>, trace: ExtensionRef<'_, RequestSpan>) -> String {
        let span = &trace.get().0;

        // `instrument` keeps the span current across this await without ever
        // holding an `Entered` guard over a suspension point.
        let total = load_total(id).instrument(span.clone()).await;

        // A synchronous emission site enters and exits within one closure.
        span.in_scope(|| {
            tracing::event!(
                name: "order.served",
                Level::INFO,
                order.id = id,
                principal.id = principal.id,
                "served an order",
            );
        });

        format!("order {id} for {} totals {total}", principal.name)
    }

    /// Opens the request span, then authenticates.
    ///
    /// The span is inserted *before* the credential check so that the `401`
    /// short-circuit below, and any later routing failure, are correlated too.
    #[before]
    async fn authenticate(&self, ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let correlation = ctx
            .get_extension::<CorrelationId>()
            .copied()
            .unwrap_or_else(|| CorrelationId(NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed)));

        let span = tracing::info_span!(
            "http.request",
            correlation = correlation.0,
            http.method = %ctx.method(),
            http.path = ctx.uri().path(),
        );

        let public = ctx.uri().path() == "/health";
        ctx.insert_extension(correlation);
        ctx.insert_extension(RequestSpan(span.clone()));

        if public {
            return Before::Next;
        }

        let Some(principal) = ctx.headers().get("authorization").and_then(bearer).and_then(account) else {
            span.in_scope(|| {
                tracing::event!(name: "auth.rejected", Level::WARN, auth.reason = "missing or unknown bearer token", "rejected a request");
            });
            return Before::Respond(StatusCode::UNAUTHORIZED);
        };

        span.in_scope(|| {
            tracing::event!(name: "auth.accepted", Level::INFO, principal.id = principal.id, principal.name = principal.name, "authenticated a caller");
        });
        ctx.insert_extension(principal);
        Before::Next
    }

    /// Records the outcome of **every generated response**: handler responses,
    /// the `401` short-circuit above, extractor rejections, and routing
    /// failures. It re-enters the span the `#[before]` stored, so the event
    /// correlates even for requests that never reached a handler.
    #[after]
    async fn record_outcome(&self, ctx: &mut AfterContext<'_>) {
        let status = ctx.status();

        if let Some(trace) = ctx.request().extensions.get::<RequestSpan>() {
            trace.0.in_scope(|| {
                tracing::event!(name: "http.response", Level::INFO, http.status = status.as_u16(), "completed a request");
            });
        }

        if let Some(correlation) = ctx.request().extensions.get::<CorrelationId>() {
            let value = correlation
                .0
                .to_string()
                .parse()
                .expect("a decimal integer is a valid header value");
            _ = ctx.headers_mut().insert(CORRELATION_HEADER, value);
        }
    }
}

/// Stands in for the I/O a real handler would await.
///
/// The yield is a real suspension point, so the event below proves the
/// `instrument`ed span survives one.
async fn load_total(id: &str) -> usize {
    tokio::task::yield_now().await;
    tracing::event!(name: "order.loaded", Level::DEBUG, order.id = id, "loaded an order");
    id.len() * 10
}

/// Extracts the credential from an `Authorization: Bearer ...` header.
fn bearer(value: &http::HeaderValue) -> Option<&str> {
    value.to_str().ok()?.strip_prefix("Bearer ")
}

/// Resolves a credential to a principal.
fn account(token: &str) -> Option<Principal> {
    ACCOUNTS.iter().enumerate().find_map(|(index, (secret, name))| {
        (*secret == token).then(|| Principal {
            id: index as u64 + 1,
            name,
        })
    })
}

/// Builds the transport stack: a Tower layer assigns the correlation id, and
/// the generated router owns authentication and tracing.
///
/// Tower is the right home for the edge concern (the id may come from a proxy
/// header and must exist before anything else runs), while the guard and the
/// response record stay generated, direct, and unboxed inside the router.
fn stack() -> impl Service<Request<Body>, Response = BoxedResponse, Error = Infallible, Future: Send> {
    let routing = RouteService::new(Orders, (), |orders: Orders, (): (), request: Request<Body>| async move {
        orders.route(request, &()).await
    })
    .send_boxed_body();

    ServiceBuilder::new()
        .layer(MapRequestLayer::new(|mut request: Request<Body>| {
            let supplied = request
                .headers()
                .get(CORRELATION_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok());
            let correlation = supplied.unwrap_or_else(|| NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed));
            _ = request.extensions_mut().insert(CorrelationId(correlation));
            request
        }))
        .service(routing)
}

/// Drives one request through the whole stack and reads the response.
async fn call(request: Request<Body>) -> (StatusCode, String, Bytes) {
    let response = stack().oneshot(request).await.expect("routing is infallible");
    let status = response.status();
    let correlation = response
        .headers()
        .get(CORRELATION_HEADER)
        .map_or_else(String::new, |value| value.to_str().unwrap_or_default().to_owned());
    let body = response.into_body().collect().await.expect("the response body succeeds").to_bytes();
    (status, correlation, body)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    // An authenticated request: the caller supplies the correlation id, the
    // guard authenticates, and the handler borrows both the principal and the
    // path capture.
    let (status, correlation, body) = call(
        Request::get("/orders/9f3")
            .header(CORRELATION_HEADER, "77")
            .header("authorization", "Bearer token-ada")
            .body(Body::empty())
            .expect("the request is valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(correlation, "77");
    assert_eq!(body, b"order 9f3 for ada totals 30"[..]);

    // An anonymous request: the guard short-circuits, and the `#[after]` still
    // records the status and stamps the correlation header.
    let (status, correlation, _) = call(Request::get("/orders/9f3").body(Body::empty()).expect("the request is valid")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!correlation.is_empty(), "the short-circuit response is still correlated");

    // An unknown credential is rejected the same way.
    let (status, ..) = call(
        Request::get("/orders/9f3")
            .header("authorization", "Bearer token-nobody")
            .body(Body::empty())
            .expect("the request is valid"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A routing failure is a generated response too, so it is traced.
    let (status, correlation, _) = call(
        Request::get("/absent")
            .header("authorization", "Bearer token-grace")
            .body(Body::empty())
            .expect("the request is valid"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!correlation.is_empty(), "the routing failure is still correlated");

    // The public probe skips authentication but is traced like everything else.
    let (status, ..) = call(Request::get("/health").body(Body::empty()).expect("the request is valid")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    println!("all authentication and tracing scenarios passed");
}
