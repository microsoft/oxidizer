// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{BeforeContext, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::OK
    }

    #[before]
    async fn guard(&self, _ctx: &mut BeforeContext<'_>) -> StatusCode {
        StatusCode::OK
    }
}

fn main() {}
