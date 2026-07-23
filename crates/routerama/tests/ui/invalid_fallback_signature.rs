// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{RouteFailure, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[fallback]
    async fn fallback(&self, _failure: &RouteFailure<'_>) -> StatusCode {
        StatusCode::NOT_FOUND
    }
}

fn main() {}
