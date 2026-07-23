// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[catch(T)]
    async fn catcher<T>(&self, _rejection: T) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

fn main() {}
