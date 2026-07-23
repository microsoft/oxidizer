// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use routerama::route::{BodyConsumed, RequestParts, StatusCode, router};

struct Api;

#[router]
impl Api {
    #[route(POST, "/")]
    async fn create(&self) -> StatusCode {
        StatusCode::OK
    }

    #[transform(stream, limit = 64, create)]
    async fn drain<B>(&self, _parts: &RequestParts, body: B) -> BodyConsumed<StatusCode>
    where
        B: http_body::Body<Data = Bytes>,
    {
        drop(body);
        BodyConsumed::Consumed
    }
}

fn main() {}
