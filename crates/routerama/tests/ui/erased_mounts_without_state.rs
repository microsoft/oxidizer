// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{StatusCode, router};

struct Api;

#[router(erased_mounts)]
impl Api {
    #[route(GET, "/")]
    async fn health(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

fn main() {}
