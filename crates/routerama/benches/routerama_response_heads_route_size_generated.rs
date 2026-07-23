// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated-router compile and section-size control with static route header plans.

use routerama::response::{Body, Response};
use routerama::route::router;

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
        Response::new(Body::empty())
    }

    #[route(GET, "/headers/1", headers(insert("x-template-00", "value-00")))]
    async fn headers_1(&self) -> Response {
        Response::new(Body::empty())
    }

    #[route(
        GET,
        "/headers/4",
        headers(
            insert("x-template-00", "value-00"),
            insert("x-template-01", "value-01"),
            insert("x-template-02", "value-02"),
            insert("x-template-03", "value-03"),
        )
    )]
    async fn headers_4(&self) -> Response {
        Response::new(Body::empty())
    }

    #[route(
        GET,
        "/headers/16",
        headers(
            insert("x-template-00", "value-00"),
            insert("x-template-01", "value-01"),
            insert("x-template-02", "value-02"),
            insert("x-template-03", "value-03"),
            insert("x-template-04", "value-04"),
            insert("x-template-05", "value-05"),
            insert("x-template-06", "value-06"),
            insert("x-template-07", "value-07"),
            insert("x-template-08", "value-08"),
            insert("x-template-09", "value-09"),
            insert("x-template-10", "value-10"),
            insert("x-template-11", "value-11"),
            insert("x-template-12", "value-12"),
            insert("x-template-13", "value-13"),
            insert("x-template-14", "value-14"),
            insert("x-template-15", "value-15"),
        )
    )]
    async fn headers_16(&self) -> Response {
        Response::new(Body::empty())
    }
}

static API: Api = Api;

include!("common/response_head_route_size_control.rs");
