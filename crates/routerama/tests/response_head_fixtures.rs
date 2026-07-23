// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavioral coverage for generated static response-header plans.

#![allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers and interceptors must be async; the trait lint is toolchain-dependent"
)]

use http::header::{CONTENT_TYPE, HeaderName, HeaderValue, SET_COOKIE};
use http::{Request, StatusCode, Version};
use routerama::response::{Body, Response};
use routerama::route::{AfterContext, router};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Marker(u64);

struct StaticHeadApi;

#[router]
impl StaticHeadApi {
    #[route(
        GET,
        "/head",
        headers(
            insert("X-Replace", "route"),
            append("set-cookie", "route-a=1"),
            append("set-cookie", "route-b=2"),
            append("x-sequence", "first"),
            insert("x-sequence", "replacement"),
            append("x-sequence", "after"),
            insert("content-type", "text/plain"),
        ),
        produces = "application/json"
    )]
    async fn head(&self) -> Response {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::CREATED;
        *response.version_mut() = Version::HTTP_2;
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-replace"), HeaderValue::from_static("handler"));
        response.headers_mut().insert(SET_COOKIE, HeaderValue::from_static("handler=0"));
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-sequence"), HeaderValue::from_static("handler"));
        response.extensions_mut().insert(Marker(7));
        response
    }

    #[after(head)]
    async fn observe_and_extend(&self, context: &mut AfterContext<'_>) {
        assert_eq!(context.status(), StatusCode::CREATED);
        assert_eq!(context.version(), Version::HTTP_2);
        assert_eq!(context.headers()["x-replace"], "route");
        assert_eq!(context.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(values(context.headers(), SET_COOKIE), ["handler=0", "route-a=1", "route-b=2"]);
        assert_eq!(
            values(context.headers(), HeaderName::from_static("x-sequence")),
            ["replacement", "after"]
        );
        assert_eq!(context.extensions().get::<Marker>(), Some(&Marker(7)));

        context
            .headers_mut()
            .insert(HeaderName::from_static("x-after"), HeaderValue::from_static("observed"));
        context.headers_mut().append(SET_COOKIE, HeaderValue::from_static("after=3"));
    }
}

static API: StaticHeadApi = StaticHeadApi;

fn request() -> Request<Body> {
    Request::get("/head").body(Body::empty()).expect("the static-head request is valid")
}

fn values<const N: usize>(headers: &http::HeaderMap, name: HeaderName) -> [&str; N] {
    let mut values = headers
        .get_all(name)
        .iter()
        .map(|value| value.to_str().expect("fixture headers are ASCII"));
    core::array::from_fn(|_| values.next().expect("the expected response-header value exists"))
}

#[tokio::test]
async fn generated_static_headers_preserve_response_and_interceptor_semantics() {
    let (mut first, second) = tokio::join!(API.route(request(), &()), API.route(request(), &()));

    for response in [&first, &second] {
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.version(), Version::HTTP_2);
        assert_eq!(response.headers()["x-replace"], "route");
        assert_eq!(response.headers()["x-after"], "observed");
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(
            values(response.headers(), SET_COOKIE),
            ["handler=0", "route-a=1", "route-b=2", "after=3"]
        );
        assert_eq!(
            values(response.headers(), HeaderName::from_static("x-sequence")),
            ["replacement", "after"]
        );
        assert_eq!(response.extensions().get::<Marker>(), Some(&Marker(7)));
    }

    first
        .headers_mut()
        .insert(HeaderName::from_static("x-independent"), HeaderValue::from_static("first"));
    assert!(second.headers().get("x-independent").is_none());
}
