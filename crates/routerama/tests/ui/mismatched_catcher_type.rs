// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{BodyRejection, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[catch(StatusCode)]
    async fn catcher(&self, _rejection: BodyRejection<core::convert::Infallible>) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

fn main() {}
