// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(unused_imports)]

use routerama::route::{StatusCode, router};

struct Borrowed<'a>(&'a str);
struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn home(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[catch(Borrowed<'_>)]
    async fn catcher(&self, rejection: Borrowed<'_>) -> StatusCode {
        let _ = rejection;
        StatusCode::BAD_REQUEST
    }
}

fn main() {}
