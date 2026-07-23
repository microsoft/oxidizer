// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/items/{id}", host = "one.example", priority = 2)]
    async fn by_id(&self, id: u32) -> StatusCode {
        let _ = id;
        StatusCode::NO_CONTENT
    }

    #[route(GET, "/items/{name}", host = "two.example", priority = 1)]
    async fn by_name(&self, name: u32) -> StatusCode {
        let _ = name;
        StatusCode::NO_CONTENT
    }
}

fn main() {}
