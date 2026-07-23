// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{AfterContext, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::OK
    }

    #[after]
    async fn stamp(&self, _ctx: &mut AfterContext<'_>) -> String {
        String::new()
    }
}

fn main() {}
