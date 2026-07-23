// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::router;

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn health(&self) {}
}

fn main() {}
