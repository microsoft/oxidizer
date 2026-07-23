// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/", produces = "application/json", priority = 10)]
    async fn first(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/", produces = "application/json", priority = 0)]
    async fn second(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
