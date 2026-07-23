// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::router;

struct Api;

#[router]
impl Api {
    #[route(GET, "/items", host = "api.example")]
    #[route(HEAD, "/items", host = "other.example")]
    async fn items(&self) -> () {}
}

fn main() {}
