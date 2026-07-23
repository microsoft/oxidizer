// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Response status, headers, extensions, and fallible response metadata.
//!
//! Run with
//! `cargo run -p routerama --example response_composition --features response`.
//!
//! `routerama::response` is independently enabled: this example turns on
//! only the `response` feature and therefore links no path matcher, no
//! procedural macro, and no query codec. Generated routers consume exactly
//! these traits, so everything shown here composes identically inside a
//! `#[router]` handler's return type.
//!
//! Two traits carry the model:
//!
//! - [`IntoResponse`] converts a value into an `http::Response` while
//!   retaining one *concrete* body type. There is no boxed body anywhere in
//!   this file.
//! - [`IntoResponseParts`] applies metadata to a body-free [`ResponseParts`]
//!   value and may fail with a typed rejection that converts through
//!   `IntoResponse` on its own.
//!
//! Tuples compose the two. The last item becomes the response, then metadata
//! is applied right to left, so the leftmost part wins a conflict. A failing
//! part short-circuits in that same order and its rejection is returned
//! whole — the partially composed success metadata and the original body are
//! dropped rather than leaked.
//!
//! [`IntoResponse`]: routerama::response::IntoResponse
//! [`IntoResponseParts`]: routerama::response::IntoResponseParts
//! [`ResponseParts`]: routerama::response::ResponseParts

use http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, HeaderName, HeaderValue};
use http::{HeaderMap, StatusCode};
use http_body_util::BodyExt as _;
use routerama::response::{Body, IntoResponse, IntoResponseParts, Response, ResponseParts};

/// A typed value attached to the response for later transport layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheDecision {
    hit: bool,
}

/// A response part that validates an entity tag before inserting it.
///
/// This is what "fallible response metadata" means: the value is checked while
/// the response is being composed, and a failure becomes its own response.
struct ETagHeader(String);

/// The typed rejection produced by [`ETagHeader`].
#[derive(Debug)]
struct InvalidETag(String);

impl IntoResponse for InvalidETag {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        let mut response = Response::new(Body::from(format!("invalid entity tag: {}", self.0)));
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        response
    }
}

impl IntoResponseParts for ETagHeader {
    type Error = InvalidETag;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        let value = HeaderValue::from_str(&self.0).map_err(|_invalid| InvalidETag(self.0))?;
        _ = response.headers_mut().insert(ETAG, value);
        Ok(response)
    }
}

/// A response part that records a typed decision in response extensions.
struct Cached(CacheDecision);

impl IntoResponseParts for Cached {
    type Error = core::convert::Infallible;

    fn into_response_parts(self, mut response: ResponseParts) -> Result<ResponseParts, Self::Error> {
        _ = response.extensions_mut().insert(self.0);
        if self.0.hit {
            _ = response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
        }
        Ok(response)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    status_and_headers().await;
    leftmost_part_wins().await;
    extensions_travel_with_the_response();
    a_failing_part_replaces_the_response().await;
    failure_short_circuits_the_parts_to_its_left().await;
}

/// A status, a header array, and a body compose without boxing.
async fn status_and_headers() {
    let response = (
        StatusCode::CREATED,
        [(HeaderName::from_static("location"), HeaderValue::from_static("/books/42"))],
        String::from("created"),
    )
        .into_response();

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["location"], "/books/42");
    // `String` supplies its own `Content-Type` when it becomes the response.
    assert_eq!(response.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
    assert_eq!(collect(response).await, b"created"[..]);
}

/// Metadata is applied right to left, so the leftmost item wins.
async fn leftmost_part_wins() {
    let mut inner = HeaderMap::new();
    _ = inner.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    let response = (
        // Applied last, and therefore authoritative.
        [(CONTENT_TYPE, HeaderValue::from_static("application/problem+json"))],
        inner,
        String::from(r#"{"status":409}"#),
    )
        .into_response();

    assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
    assert_eq!(response.headers().get_all(CONTENT_TYPE).iter().count(), 1);
    assert_eq!(collect(response).await, br#"{"status":409}"#[..]);
}

/// Extensions are ordinary response metadata: typed, and never serialized.
fn extensions_travel_with_the_response() {
    let response = (Cached(CacheDecision { hit: true }), "cached body").into_response();

    assert_eq!(response.headers()[CACHE_CONTROL], "public, max-age=60");
    assert_eq!(response.extensions().get::<CacheDecision>(), Some(&CacheDecision { hit: true }));

    let response = (Cached(CacheDecision { hit: false }), "fresh body").into_response();
    assert!(!response.headers().contains_key(CACHE_CONTROL));
    assert_eq!(response.extensions().get::<CacheDecision>(), Some(&CacheDecision { hit: false }));
}

/// A rejected part discards the success body and returns its own response.
async fn a_failing_part_replaces_the_response() {
    let ok = (ETagHeader(String::from("\"v1\"")), String::from("body")).into_response();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[ETAG], "\"v1\"");
    assert_eq!(collect(ok).await, b"body"[..]);

    // A header value cannot contain a newline, so the part rejects.
    let failed = (ETagHeader(String::from("v1\nv2")), String::from("body")).into_response();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!failed.headers().contains_key(ETAG));
    assert_eq!(collect(failed).await, b"invalid entity tag: v1\nv2"[..]);
}

/// In a three-item tuple, the failure of the right part skips the left one.
async fn failure_short_circuits_the_parts_to_its_left() {
    let failed = (StatusCode::CREATED, ETagHeader(String::from("v1\nv2")), String::from("body")).into_response();

    // `StatusCode::CREATED` never ran: the rejection response is returned
    // whole, without the outer status or any partially applied header.
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!failed.headers().contains_key(ETAG));
    assert_eq!(collect(failed).await, b"invalid entity tag: v1\nv2"[..]);
}

async fn collect<B>(response: Response<B>) -> bytes::Bytes
where
    B: http_body::Body<Data = bytes::Bytes>,
    B::Error: core::fmt::Debug,
{
    response
        .into_body()
        .collect()
        .await
        .expect("every body in this example is infallible")
        .to_bytes()
}
