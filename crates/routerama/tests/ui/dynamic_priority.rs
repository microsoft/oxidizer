// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(dynamic, priority = 1)]
    async fn dynamic(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
