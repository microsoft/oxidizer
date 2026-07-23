// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
