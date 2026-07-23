// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::resolve::resolver;

#[resolver]
enum Route {
    #[route(GET, "/", host = "api.example")]
    Home,
}

fn main() {}
