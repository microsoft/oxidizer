// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/", host = "one.example", priority = 1)]
    async fn one(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/", host = "two.example")]
    async fn two(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
