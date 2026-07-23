// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed header access and caller-owned parse caching.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http_body_util::BodyExt as _;
use routerama::response::{Body, Response};
use routerama::route::header::{Encoding, HeaderCache, HeaderExt as _};
use routerama::route::{HeaderMap, Request, StatusCode, router};

struct Origin;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Origin {
    /// Negotiates a content coding from the client's `Accept-Encoding`.
    #[route(GET, "/download/{name}")]
    async fn download(&self, name: &str, headers: &HeaderMap) -> String {
        let supported = [Encoding::Brotli, Encoding::Gzip, Encoding::Identity];
        let chosen = match headers.accept_encoding() {
            Some(accept) => accept.preferred(supported),
            None if !headers.contains_key(http::header::ACCEPT_ENCODING) => supported.first().copied(),
            None => None,
        };
        let Some(chosen) = chosen else {
            return format!("{name} has no acceptable coding");
        };
        format!("{name} as {chosen:?}")
    }

    /// Answers a conditional `GET` by comparing `If-Modified-Since` against the
    /// resource's last-modified date.
    #[route(GET, "/articles/{id}")]
    async fn article(&self, id: u32, headers: &HeaderMap) -> Response<Body> {
        let last_modified = article_last_modified();

        if headers.if_modified_since().is_some_and(|since| !since.is_modified(last_modified)) {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_MODIFIED;
            return response;
        }

        Response::new(Body::from(format!("article {id}")))
    }

    /// Reflects the request's `Cache-Control` directives as typed values.
    #[route(GET, "/echo-cache-control")]
    async fn echo_cache_control(&self, headers: &HeaderMap) -> String {
        let Some(control) = headers.cache_control() else {
            return String::from("absent");
        };
        format!(
            "no_store={} public={} max_age={:?}",
            control.no_store(),
            control.public(),
            control.max_age().map(|age| age.as_secs())
        )
    }
}

/// A per-worker request loop that owns one [`HeaderCache`].
///
/// The cache is threaded through every request the worker handles, so repeated
/// `Date` and `Accept-Encoding` values are parsed only once.
fn worker_loop() {
    let mut cache = HeaderCache::new();

    for _request in 0..16 {
        let headers = upstream_headers();

        let served_at: SystemTime = cache.date(&headers).expect("upstream sends a valid Date").into();
        assert_eq!(served_at, article_last_modified());

        let encoding = cache
            .accept_encoding(&headers)
            .and_then(|accept| accept.preferred([Encoding::Brotli, Encoding::Gzip]))
            .expect("client accepts a supported coding");
        assert_eq!(encoding, Encoding::Brotli);
    }
}

/// Builds the header set a worker repeatedly sees from one upstream.
fn upstream_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    _ = headers.insert("date", "Sun, 06 Nov 1994 08:49:37 GMT".parse().expect("valid Date value"));
    _ = headers.insert("accept-encoding", "br, gzip;q=0.8".parse().expect("valid Accept-Encoding value"));
    headers
}

fn article_last_modified() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(784_111_777)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let response = Origin
        .route(
            Request::get("/download/report.tar")
                .header("accept-encoding", "gzip, br;q=1.0")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"report.tar as Brotli"[..]);

    let response = Origin
        .route(
            Request::get("/download/report.tar")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"report.tar as Brotli"[..]);

    let response = Origin
        .route(
            Request::get("/download/report.tar")
                .header("accept-encoding", "identity;q=0, *;q=0")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"report.tar has no acceptable coding"[..]);

    // Conditional GET: the client already has a copy at least as new as the
    // origin's, so the origin answers 304 with no body.
    let response = Origin
        .route(
            Request::get("/articles/42")
                .header("if-modified-since", "Sun, 06 Nov 1994 08:49:37 GMT")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert!(body(response).await.is_empty());

    // A stale (older) `If-Modified-Since` gets the full representation back.
    let response = Origin
        .route(
            Request::get("/articles/42")
                .header("if-modified-since", "Sat, 05 Nov 1994 08:49:37 GMT")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"article 42"[..]);

    // Typed `Cache-Control`: boolean directives and delta-seconds arguments are
    // read without touching the raw string.
    let response = Origin
        .route(
            Request::get("/echo-cache-control")
                .header("cache-control", "public, max-age=600")
                .body(Body::empty())
                .expect("static request metadata is valid"),
            &(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, b"no_store=false public=true max_age=Some(600)"[..]);

    // The per-worker cache serves repeated header values without re-parsing.
    worker_loop();
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
