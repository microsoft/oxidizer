// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{Before, BeforeContext, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    #[before]
    async fn home(&self, _ctx: &mut BeforeContext<'_>) -> Before<StatusCode> {
        Before::Next
    }
}

fn main() {}
