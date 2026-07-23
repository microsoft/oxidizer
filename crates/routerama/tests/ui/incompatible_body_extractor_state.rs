// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use routerama::route::{
    BodyStateWitness, FromRequestBody, RequestParts, StatusCode, router,
};

struct AppState;
struct OtherState;
struct OtherStateBody;

impl FromRequestBody<OtherState, Vec<u8>> for OtherStateBody {
    type Rejection = StatusCode;

    fn from_request_body(
        _parts: &RequestParts,
        _body: Vec<u8>,
        _state: &OtherState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> {
        core::future::ready(Ok(Self))
    }
}

impl BodyStateWitness<AppState, StatusCode> for OtherStateBody {
    type RequestBody = Vec<u8>;
}

struct Api;

#[router(state = AppState)]
impl Api {
    #[route(POST, "/")]
    async fn home(&self, #[body] body: OtherStateBody) -> StatusCode {
        let _ = body;
        StatusCode::NO_CONTENT
    }
}

fn main() {}
