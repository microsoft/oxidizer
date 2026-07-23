// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use routerama::route::{BodyConsumed, BytesBody, RequestParts, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(POST, "/")]
    async fn create(&self, #[body] data: BytesBody<64>) -> StatusCode {
        let _ = data;
        StatusCode::OK
    }

    #[transform(stream, create)]
    async fn drain<B>(&self, _parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
    where
        B: http_body::Body<Data = Bytes>,
    {
        drop(body);
        BodyConsumed::Consumed
    }
}

fn main() {}
