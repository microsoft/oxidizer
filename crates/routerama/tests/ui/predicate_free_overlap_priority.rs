// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/", priority = 10)]
    async fn anything(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/", produces = "application/json", priority = 0)]
    async fn json(&self) -> &'static str {
        "{}"
    }
}

fn main() {}
