// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn health() -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
