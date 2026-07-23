// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use routerama::route::{BytesBody, RequestParts, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(POST, "/")]
    async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
        let _ = data;
        StatusCode::OK
    }

    #[transform(limit = 64, create)]
    async fn shrink(&self, _parts: &RequestParts, body: Bytes) -> StatusCode {
        let _ = body;
        StatusCode::OK
    }
}

fn main() {}
