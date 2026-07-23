// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated-router compile and section-size control with ordinary handler insertion.

use http::header::{HeaderName, HeaderValue};
use routerama::response::{Body, Response};
use routerama::route::router;

const HEADER_FIELDS: [(&str, &str); 16] = [
    ("x-template-00", "value-00"),
    ("x-template-01", "value-01"),
    ("x-template-02", "value-02"),
    ("x-template-03", "value-03"),
    ("x-template-04", "value-04"),
    ("x-template-05", "value-05"),
    ("x-template-06", "value-06"),
    ("x-template-07", "value-07"),
    ("x-template-08", "value-08"),
    ("x-template-09", "value-09"),
    ("x-template-10", "value-10"),
    ("x-template-11", "value-11"),
    ("x-template-12", "value-12"),
    ("x-template-13", "value-13"),
    ("x-template-14", "value-14"),
    ("x-template-15", "value-15"),
];

fn response(count: usize) -> Response {
    let mut response = Response::new(Body::empty());
    for &(name, value) in &HEADER_FIELDS[..count] {
        response
            .headers_mut()
            .insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    response
}

struct Api;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Api {
    #[route(GET, "/headers/0")]
    async fn headers_0(&self) -> Response {
        response(0)
    }

    #[route(GET, "/headers/1")]
    async fn headers_1(&self) -> Response {
        response(1)
    }

    #[route(GET, "/headers/4")]
    async fn headers_4(&self) -> Response {
        response(4)
    }

    #[route(GET, "/headers/16")]
    async fn headers_16(&self) -> Response {
        response(16)
    }
}

static API: Api = Api;

include!("common/response_head_route_size_control.rs");
