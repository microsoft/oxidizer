// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serving a generated REST service from an `axum` application.
//!
//! `axum` is built on `tower` + `http`, so the generated [`Transcoder`] plugs in
//! as a fallback [`tower_service::Service`] with no hand-written handler:
//! [`RestService::new`](rest_over_grpc::serving::RestService::new) wraps the
//! transcoder, and `axum`'s `fallback_service` mounts it directly. The service
//! reads each request, routes it through the build-time-generated router, and
//! returns the response — a unary RPC buffered, a server-streaming RPC as a live
//! frame stream — so one fallback serves both shapes.
//!
//! The streaming service returns a [`Response`](http::Response) whose body is an
//! [`http_body::Body`], which is exactly what `axum` mounts; no `IntoResponse`
//! glue or manual body conversion is needed. (For a handler that must construct
//! responses itself — mixing transcoded and hand-built replies — the `rest_over_grpc`
//! `axum` feature additionally implements
//! [`IntoResponse`](https://docs.rs/axum/latest/axum/response/trait.IntoResponse.html) for the
//! neutral response types.)
//!
//! In production you would serve the router over TCP:
//!
//! ```ignore
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
//! axum::serve(listener, app()).await?;
//! ```
//!
//! To keep the example self-contained and deterministic, it drives the *real*
//! `axum::Router` with `tower`'s `oneshot` instead of binding a socket.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example axum_app
//! ```

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use http::header::ACCEPT;
use rest_over_grpc::serving::RestService;
use rest_over_grpc_examples::custom::{InMemoryLibrary, Transcoder};
use tower::ServiceExt as _;

/// A cap on the buffered request body, mirroring what a real deployment would
/// enforce rather than reading an unbounded body into memory.
const MAX_BODY: usize = 64 * 1024;

/// A demo request driven through the router: `(label, method, target, optional
/// Accept header, body)`.
type DemoRequest = (&'static str, &'static str, &'static str, Option<&'static str>, &'static [u8]);

/// Builds the `axum` application: the generated transcoder, wrapped as a
/// streaming [`tower_service::Service`], mounted as the router's fallback so the
/// build-time-generated router owns REST routing. `Arc` lets the single
/// transcoder instance be shared/cloned into the service per request (the
/// generated `Transcoder` owns the handlers and is `Clone` when they are).
fn app() -> Router {
    let transcoder = Arc::new(Transcoder::new(InMemoryLibrary));
    Router::new().fallback_service(RestService::new(transcoder).with_max_body_bytes(MAX_BODY))
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // (label, method, target, accept, body)
    let requests: [DemoRequest; 6] = [
        ("path variable", "GET", "/v1/shelves/history", None, b""),
        ("query parameter", "GET", "/v1/shelves?filter=science", None, b""),
        ("enum path variable", "GET", "/v1/shelves/genre/SCIENCE", None, b""),
        ("request body", "POST", "/v1/shelves", None, br#"{"theme":"mystery"}"#),
        (
            "server-streaming (NDJSON)",
            "GET",
            "/v1/shelves:stream",
            Some("application/x-ndjson"),
            b"",
        ),
        ("unknown route", "GET", "/nope", None, b""),
    ];

    for (label, method, target, accept, body) in requests {
        let mut builder = http::Request::builder().method(method).uri(target);
        if let Some(accept) = accept {
            builder = builder.header(ACCEPT, accept);
        }
        let request = builder.body(Body::from(body.to_vec())).expect("request builds");

        // `oneshot` drives the real router (routing, the fallback service, its
        // response body) exactly as a live server would, per request.
        let response = app().oneshot(request).await.expect("transcode is infallible");

        let status = response.status();
        let content_type = response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let bytes = to_bytes(response.into_body(), MAX_BODY).await.expect("response body collects");

        println!(
            "{label}: {method} {target} -> {} [{content_type}] {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
        );
    }
}
