// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serving a generated `routerama` router as a [`tower_service::Service`].
//!
//! Run it with `cargo run --example tower_service --features tower`.
//!
//! `RouteService` turns any routing call into a Tower service, so ordinary
//! Tower layers compose above it and any Tower-compatible transport can drive
//! it. This example builds one stack:
//!
//! ```text
//! axum transport
//!   +- ConcurrencyLimit    (real readiness, above an always-ready router)
//!      +- MapRequest       (attaches a caller identity extension)
//!         +- MapResponse   (stamps a response header)
//!            +- RouteService(generated routing, one boxed response body)
//! ```
//!
//! The layer-supplied extension is consumed by a generated `#[before]`
//! interceptor, which promotes it to a typed value the handler extracts. That
//! is the intended division of labour: Tower owns runtime-open middleware at
//! the transport boundary, while generated interceptors stay direct, concrete,
//! and unboxed inside the router.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http_body_util::BodyExt as _;
use routerama::response::SendBoxBody;
use routerama::route::tower::RouteService;
use routerama::route::{Before, BeforeContext, ExtensionRef, Request, State, StatusCode, router};
use tokio::net::TcpListener;
use tower::util::{MapRequestLayer, MapResponseLayer};
use tower::{ServiceBuilder, ServiceExt as _};
use tower_service::Service;

type Response = routerama::response::Response<SendBoxBody>;

/// The caller identity a Tower layer attaches to every request.
#[derive(Clone, Copy)]
struct Caller(&'static str);

/// The identity the router-wide `#[before]` guard promotes for handlers.
#[derive(Clone, Copy)]
struct Authenticated(&'static str);

/// Shared, read-only application state.
#[derive(Clone)]
struct AppState {
    deployment: &'static str,
}

/// A zero-sized router value: cloning it into each Tower call costs nothing.
#[derive(Clone, Copy)]
struct Books;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router(state = AppState)]
impl Books {
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/books/{id}")]
    async fn book(&self, id: u32, caller: ExtensionRef<'_, Authenticated>, state: State<AppState>) -> String {
        format!("{}/{}: book {id}\n", state.deployment, caller.0.0)
    }

    /// A generated router-wide guard that reads what the Tower layer inserted.
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

/// Builds the complete Tower stack.
///
/// The returned service is `Clone`, so a transport may clone it per connection
/// or per request. Cloning shares the one `Arc` state; the router itself is
/// zero-sized.
fn stack()
-> impl Service<Request<Body>, Response = Response, Error = core::convert::Infallible, Future: Send> + Clone + Send + Sync + 'static {
    // `RouteService` hands the callable owned clones, so its future borrows
    // nothing from the service and satisfies Tower's associated future type.
    let routing = RouteService::new(
        Books,
        Arc::new(AppState { deployment: "west" }),
        |books: Books, state: Arc<AppState>, request: Request<Body>| async move { books.route(request, &state).await },
    )
    // One allocation per response buys a nameable `Send + 'static` body, which
    // is what the transport below needs. Without this the response body stays
    // the router's own unboxed concrete sum.
    .send_boxed_body();

    ServiceBuilder::new()
        .concurrency_limit(64)
        .layer(MapRequestLayer::new(|mut request: Request<Body>| {
            let _ = request.extensions_mut().insert(Caller("demo"));
            request
        }))
        .layer(MapResponseLayer::new(|mut response: Response| {
            let _ = response
                .headers_mut()
                .insert("x-served-by", "routerama".parse().expect("the static header value is valid"));
            response
        }))
        .service(routing)
}

#[tokio::main]
async fn main() {
    // Drive the stack in process first: `oneshot` waits for readiness and then
    // calls, which is exactly what a transport does per request.
    let response = stack()
        .oneshot(Request::get("/books/42").body(Body::empty()).expect("the request is valid"))
        .await
        .expect("routing is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-served-by"], "routerama");
    let body = response.into_body().collect().await.expect("the response body succeeds").to_bytes();
    assert_eq!(body, b"west/demo: book 42\n"[..]);
    println!("in-process: {}", String::from_utf8_lossy(&body).trim_end());

    // The same value is a transport-ready service: `axum` only carries bytes.
    let app = Router::new().fallback_service(stack());
    let listener = TcpListener::bind("127.0.0.1:8081").await.expect("failed to bind 127.0.0.1:8081");
    println!("listening on http://127.0.0.1:8081 (try /health and /books/42)");
    let server = axum::serve(listener, app);

    if std::env::var_os("IS_TESTING").is_some() {
        server.with_graceful_shutdown(async {}).await.expect("server error");
    } else {
        server.await.expect("server error");
    }
}
