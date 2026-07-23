// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::resolve::resolver;

#[resolver]
enum Route {
    #[route(GET, "/", priority = 1)]
    Home,
}

fn main() {}
