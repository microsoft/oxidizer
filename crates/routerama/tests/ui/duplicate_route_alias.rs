// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/items", priority = 1)]
    #[route(GET, "/items", priority = 0)]
    async fn items(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
