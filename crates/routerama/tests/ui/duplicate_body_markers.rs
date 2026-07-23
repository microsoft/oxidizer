// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::router;

struct Api;

#[router]
impl Api {
    #[route(POST, "/")]
    async fn create(
        &self,
        #[body] first: Vec<u8>,
        #[body] second: Vec<u8>,
    ) -> () {
        let _ = (first, second);
    }
}

fn main() {}
