// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{AfterContext, Before, BeforeContext, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::OK
    }

    #[before]
    #[after]
    async fn both(&self, _ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        let _: fn(&mut AfterContext<'_>) = |_| {};
        Before::Next
    }
}

fn main() {}
