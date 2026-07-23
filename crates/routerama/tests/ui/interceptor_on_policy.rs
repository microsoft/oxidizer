// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{AfterContext, RouteFailure, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[fallback]
    #[after]
    async fn fallback(&self, failure: RouteFailure<'_>) -> StatusCode {
        let _ = failure;
        StatusCode::NOT_FOUND
    }
}

fn main() {}
