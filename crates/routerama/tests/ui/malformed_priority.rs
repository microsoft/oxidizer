// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/", priority = "high")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
