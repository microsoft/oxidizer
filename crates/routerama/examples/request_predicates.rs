// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Host, `Content-Type`, and `Accept` route predicates and overlap priority.

use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::{Request, StatusCode, router};

struct Catalog;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Catalog {
    /// Accepts JSON uploads for one authority and answers in JSON.
    #[route(
        POST,
        "/catalog/{id}",
        host = "catalog.example",
        consumes = "application/json",
        produces = "application/json"
    )]
    async fn upsert(&self, id: u32) -> String {
        format!(r#"{{"id":{id}}}"#)
    }

    /// Negotiates between two representations of the same resource.
    #[route(GET, "/catalog/{id}", produces = "text/html", priority = 10)]
    async fn html(&self, id: u32) -> String {
        format!("<p>{id}</p>")
    }

    #[route(GET, "/catalog/{id}", produces = "text/plain", priority = 0)]
    async fn text(&self, id: u32) -> String {
        format!("item {id}")
    }

    /// Declares no predicate, so no predicate work is generated for it.
    #[route(GET, "/health")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // The authority may come from the URI, and matching is case-insensitive.
    let accepted = Catalog
        .route(
            Request::post("https://CATALOG.EXAMPLE/catalog/7")
                .header("content-type", "application/json; charset=utf-8")
                .header("accept", "application/*")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(accepted.headers()["content-type"], "application/json");
    assert_eq!(body(accepted).await, br#"{"id":7}"#[..]);

    // ... or from the `Host` header when the URI is origin-form.
    let via_header = Catalog
        .route(
            Request::post("/catalog/7")
                .header("host", "catalog.example")
                .header("content-type", "application/json")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(via_header.status(), StatusCode::OK);

    // A different authority does not route here at all.
    let wrong_host = Catalog
        .route(
            Request::post("/catalog/7")
                .header("host", "other.example")
                .header("content-type", "application/json")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(wrong_host.status(), StatusCode::NOT_FOUND);

    // The right authority with the wrong request media type is 415.
    let wrong_media = Catalog
        .route(
            Request::post("/catalog/7")
                .header("host", "catalog.example")
                .header("content-type", "text/plain")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(wrong_media.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Acceptability is a per-route predicate, and overlapping candidates are
    // tried from the highest declared `priority` down. `text/html` is
    // acceptable at `q=0.2`, so the higher-priority route still wins: quality
    // does not reorder candidates.
    let negotiated = Catalog
        .route(
            Request::get("/catalog/7")
                .header("accept", "text/html;q=0.2, text/plain;q=0.9")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(negotiated.status(), StatusCode::OK);
    assert_eq!(negotiated.headers()["content-type"], "text/html");
    assert_eq!(body(negotiated).await, b"<p>7</p>"[..]);

    // Refusing the higher-priority representation outright moves selection on
    // to the next candidate. A more specific range overrides a broader one,
    // so the explicit `q=0` beats the trailing wildcard.
    let plain = Catalog
        .route(
            Request::get("/catalog/7")
                .header("accept", "text/html;q=0, */*")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(plain.status(), StatusCode::OK);
    assert_eq!(plain.headers()["content-type"], "text/plain");
    assert_eq!(body(plain).await, b"item 7"[..]);

    // When no candidate is acceptable, the deepest stage any candidate reached
    // decides the status: here both reached `produces`, so it is 406.
    let refused = Catalog
        .route(
            Request::get("/catalog/7")
                .header("accept", "text/html;q=0, text/plain;q=0, */*;q=0.1")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::NOT_ACCEPTABLE);

    // No `Accept` header accepts whatever the highest-priority route produces.
    let unspecified = Catalog
        .route(Request::get("/catalog/7").body(Body::empty()).expect("valid request"), &())
        .await;
    assert_eq!(unspecified.headers()["content-type"], "text/html");

    // A route without predicates ignores all of this.
    let health = Catalog
        .route(
            Request::get("/health")
                .header("accept", "application/vnd.example")
                .body(Body::empty())
                .expect("valid request"),
            &(),
        )
        .await;
    assert_eq!(health.status(), StatusCode::NO_CONTENT);
}

async fn body<B>(response: Response<B>) -> bytes::Bytes
where
    B: http_body::Body<Data = bytes::Bytes>,
    B::Error: core::fmt::Debug,
{
    response
        .into_body()
        .collect()
        .await
        .expect("example response bodies succeed")
        .to_bytes()
}
