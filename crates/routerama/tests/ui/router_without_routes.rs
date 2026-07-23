// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::router;

struct Api;

#[router]
impl Api {
    fn helper(&self) -> u32 {
        7
    }
}

fn main() {}
