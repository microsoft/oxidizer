// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use routerama::route::{BodyTransform, BytesBody, RequestParts, router};

struct Api;

#[router]
impl Api {
    #[route(POST, "/")]
    async fn create(&self, #[body] data: BytesBody<64>) -> &'static str {
        let _ = data;
        "created"
    }

    #[transform(stream, create)]
    async fn wrap<B>(&self, _parts: &RequestParts, body: B) -> BodyTransform<B, B>
    where
        B: http_body::Body<Data = Bytes>,
    {
        BodyTransform::Replace(body)
    }
}

fn main() {}
