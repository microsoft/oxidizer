// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{Before, SelectedContext, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::OK
    }

    #[before]
    async fn guard(&self, _ctx: &mut SelectedContext<'_>) -> Before<StatusCode> {
        Before::Next
    }
}

fn main() {}
