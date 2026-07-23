// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::router;

struct Api;

#[router(tower)]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> routerama::route::StatusCode {
        routerama::route::StatusCode::NO_CONTENT
    }
}

fn main() {}
