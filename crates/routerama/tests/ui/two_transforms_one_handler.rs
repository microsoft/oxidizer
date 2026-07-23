// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use routerama::response::Body;
use routerama::route::{BodyTransform, BytesBody, RequestParts, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(POST, "/")]
    async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
        let _ = data;
        StatusCode::OK
    }

    #[transform(limit = 64, create)]
    async fn first(&self, _parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        BodyTransform::Replace(Body::from_bytes(body))
    }

    #[transform(limit = 64, create)]
    async fn second(&self, _parts: &RequestParts, body: Bytes) -> BodyTransform<Body, StatusCode> {
        BodyTransform::Replace(Body::from_bytes(body))
    }
}

fn main() {}
