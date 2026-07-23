// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    unused_imports,
    reason = "the macro rejects the handler before its imported parameter and response types are emitted"
)]

use routerama::route::{HeaderMap, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(GET, "/")]
    async fn invalid(&self, headers: &'static HeaderMap) -> StatusCode {
        let _ = headers;
        StatusCode::NO_CONTENT
    }
}

fn main() {}
